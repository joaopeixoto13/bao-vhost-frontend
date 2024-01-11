// Copyright (c) Bao Project and Contributors. All rights reserved.
//          João Peixoto <joaopeixotooficial@gmail.com>
//
// SPDX-License-Identifier: Apache-2.0

//! The 'MMIO' module serves as an abstraction to implement the device MMIO functionalities.
//! These functionalities aim to implement the control plane on the frontend and cover:
//!
//! - Device configuration space operations.
//! - Device write and read operations.

use super::{device::BaoDevice, guest::BaoGuest};
use bao_sys::{defines::*, error::*, types::*};
use libc::{MAP_SHARED, PROT_READ, PROT_WRITE};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use vhost::vhost_user::message::{VhostUserProtocolFeatures, VHOST_USER_CONFIG_OFFSET};
use vhost_user_frontend::{Generic, GuestMemoryMmap, GuestRegionMmap, VirtioDevice};
use virtio_bindings::virtio_config::{VIRTIO_F_IOMMU_PLATFORM, VIRTIO_F_VERSION_1};
use virtio_bindings::virtio_mmio::{
    VIRTIO_MMIO_CONFIG_GENERATION, VIRTIO_MMIO_DEVICE_FEATURES, VIRTIO_MMIO_DEVICE_FEATURES_SEL,
    VIRTIO_MMIO_DEVICE_ID, VIRTIO_MMIO_DRIVER_FEATURES, VIRTIO_MMIO_DRIVER_FEATURES_SEL,
    VIRTIO_MMIO_INTERRUPT_ACK, VIRTIO_MMIO_INTERRUPT_STATUS, VIRTIO_MMIO_INT_VRING,
    VIRTIO_MMIO_MAGIC_VALUE, VIRTIO_MMIO_QUEUE_AVAIL_HIGH, VIRTIO_MMIO_QUEUE_AVAIL_LOW,
    VIRTIO_MMIO_QUEUE_DESC_HIGH, VIRTIO_MMIO_QUEUE_DESC_LOW, VIRTIO_MMIO_QUEUE_NOTIFY,
    VIRTIO_MMIO_QUEUE_NUM, VIRTIO_MMIO_QUEUE_NUM_MAX, VIRTIO_MMIO_QUEUE_READY,
    VIRTIO_MMIO_QUEUE_SEL, VIRTIO_MMIO_QUEUE_USED_HIGH, VIRTIO_MMIO_QUEUE_USED_LOW,
    VIRTIO_MMIO_STATUS, VIRTIO_MMIO_VENDOR_ID, VIRTIO_MMIO_VERSION,
};
use virtio_queue::{Queue, QueueT};
use vm_memory::{
    guest_memory::FileOffset, ByteValued, GuestAddress, GuestMemoryAtomic, MmapRegion,
};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

/// Struct representing a Virtqueue.
///
/// # Attributes
///
/// * `ready` - MMIO Queue Ready
/// * `size` - MMIO Queue Size
/// * `size_max` - MMIO Queue Max Size
/// * `desc_lo` - MMIO Queue Descriptor Area Low
/// * `desc_hi` - MMIO Queue Descriptor Area High
/// * `avail_lo` - MMIO Queue Available Area Low
/// * `avail_hi` - MMIO Queue Available Area High
/// * `used_lo` - MMIO Queue Used Area Low
/// * `used_hi` - MMIO Queue Used Area High
/// * `kick` - MMIO Queue Notify
struct VirtQueue {
    ready: u32,
    size: u32,
    size_max: u32,
    desc_lo: u32,
    desc_hi: u32,
    avail_lo: u32,
    avail_hi: u32,
    used_lo: u32,
    used_hi: u32,
    kick: EventFd,
}

/// Struct representing a Bao MMIO.
///
/// # Attributes
///
/// * `addr` - MMIO Address
/// * `magic` - MMIO Magic Value
/// * `version` - MMIO Version
/// * `vendor_id` - MMIO Vendor ID
/// * `status` - MMIO Status
/// * `queue_sel` - MMIO Queue Select
/// * `device_features_sel` - MMIO Device Features Select
/// * `driver_features` - MMIO Driver Features
/// * `driver_features_sel` - MMIO Driver Features Select
/// * `interrupt_state` - MMIO Interrupt State
/// * `queues_count` - MMIO Queues Count
/// * `queues` - MMIO Queues
/// * `vq` - MMIO Virtqueues
/// * `regions` - Memory Regions
/// * `guest` - Associated BaoGuest object
pub struct BaoMmio {
    addr: u64,
    magic: [u8; 4],
    version: u8,
    vendor_id: u32,
    status: u32,
    queue_sel: u32,
    device_features_sel: u32,
    driver_features: u64,
    driver_features_sel: u32,
    interrupt_state: u32,
    queues_count: usize,
    queues: Vec<(usize, Queue, EventFd)>,
    vq: Vec<VirtQueue>,
    regions: Vec<GuestRegionMmap>,
    guest: Arc<BaoGuest>,
}

impl BaoMmio {
    /// Constructor function for BaoMmio.
    ///
    /// # Arguments
    ///
    /// * `gdev` - The generic vhost-user frontend object associated with the device.
    /// * `guest` - BaoGuest object.
    /// * `addr` - MMIO base address.
    /// * `ram_addr` - Guest RAM address to configure the memory region.
    /// * `ram_size` - Guest RAM size to configure the memory region.
    ///
    /// # Returns
    ///
    /// * `Result<Self>` - Result.
    pub fn new(
        gdev: &Generic,
        guest: Arc<BaoGuest>,
        addr: u64,
        ram_addr: u64,
        ram_size: u64,
    ) -> Result<Self> {
        // Get the maximum queue sizes.
        let sizes = gdev.queue_max_sizes();

        // Create the BaoMmio device.
        let mut mmio = Self {
            addr,
            magic: [b'v', b'i', b'r', b't'],
            version: 2,
            vendor_id: 0x4d564b4c,
            status: 0,
            queue_sel: 0,
            device_features_sel: 0,
            driver_features: 0,
            driver_features_sel: 0,
            interrupt_state: 0,
            queues_count: sizes.len(),
            queues: Vec::with_capacity(sizes.len()),
            vq: Vec::new(),
            regions: Vec::new(),
            guest: guest.clone(),
        };

        // Create the virtqueues.
        for (index, size) in sizes.iter().enumerate() {
            let kick = EventFd::new(EFD_NONBLOCK).unwrap();

            // Create a BaoIoEventFd struct.
            // With QEMU we only need one for all, because QEMU only sets one ioeventfd per memory listener.
            // However, with this approach (vhost-user), we need to create a new ioeventfd for each queue
            // and register it with the guest. For that reason, we must use the `BAO_IOEVENTFD_FLAG_DATAMATCH` flag and
            // pass the index of the virtqueue to the `data` field to match with the `value` field of the
            // `bao_io_request` struct inside the bao hypervisor service module.
            let ioeventfd = BaoIoEventFd {
                fd: kick.as_raw_fd() as u32,
                flags: BAO_IOEVENTFD_FLAG_DATAMATCH, // Allow a eventfd per Virtqueue
                addr: addr + VIRTIO_MMIO_QUEUE_NOTIFY as u64,
                len: 4,
                reserved: 0,
                data: index as u64, // Index of the Virtqueue to match with the 'value' field of the 'bao_io_request' struct
            };

            // Register the kick eventfd.
            match guest.dm.lock().unwrap().create_ioeventfd(ioeventfd) {
                Ok(_) => (),
                Err(err) => return Err(err),
            }

            // Create the virtqueue.
            mmio.vq.push(VirtQueue {
                ready: 0,
                size: 0,
                size_max: *size as u32,
                desc_lo: 77,
                desc_hi: 0,
                avail_lo: 0,
                avail_hi: 0,
                used_lo: 0,
                used_hi: 0,
                kick,
            });
        }

        // Map the region.
        // The start address of the region is zero because the memory region is already offseted by the
        // 'ram_addr' parameter. Providing a non-zero start address with a zero offset will allow a
        // guest to access memory that does not belong to them and that was not previously allocated
        // by the Bao hypervisor.
        match mmio.map_region(GuestAddress(0), "/dev/mem", ram_addr, ram_size as usize) {
            Ok(_) => (),
            Err(err) => return Err(err),
        }

        // Return the BaoMmio.
        Ok(mmio)
    }

    /// Method to read from the device configuration space.
    ///
    /// # Arguments
    ///
    /// * `req` - BaoIoRequest object.
    /// * `gdev` - The generic vhost-user frontend object associated with the device.
    /// * `offset` - Offset of the I/0 access.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn config_read(&self, req: &mut BaoIoRequest, gdev: &Generic, offset: u64) -> Result<()> {
        let mut data: u64 = 0;
        // Read the data from the device configuration space.
        gdev.read_config(
            offset,
            &mut data.as_mut_slice()[0..req.access_width as usize],
        );
        // Set the data to the request.
        req.value = data;
        Ok(())
    }

    /// Method to write to the device configuration space.
    ///
    /// # Arguments
    ///
    /// * `req` - BaoIoRequest object.
    /// * `gdev` - The generic vhost-user frontend object associated with the device.
    /// * `offset` - Offset of the I/0 access.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn config_write(&self, req: &mut BaoIoRequest, gdev: &mut Generic, offset: u64) -> Result<()> {
        // Write the data to the device configuration space.
        gdev.write_config(
            offset,
            &req.value.to_ne_bytes()[0..req.access_width as usize],
        );
        Ok(())
    }

    /// Method to perform an I/O read operation.
    ///
    /// # Arguments
    ///
    /// * `req` - BaoIoRequest object.
    /// * `dev` - BaoDevice object.
    /// * `offset` - Offset of the I/0 access.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn io_read(&self, req: &mut BaoIoRequest, dev: &BaoDevice, offset: u64) -> Result<()> {
        // Get the virtqueue.
        let vq = &self.vq[self.queue_sel as usize];
        // Get the generic device.
        let gdev = dev.gdev.lock().unwrap();

        // Read the data from the device by writing it to the request value.
        req.value = match offset as u32 {
            VIRTIO_MMIO_MAGIC_VALUE => u32::from_le_bytes(self.magic),
            VIRTIO_MMIO_VERSION => self.version as u32,
            VIRTIO_MMIO_DEVICE_ID => gdev.device_type(),
            VIRTIO_MMIO_VENDOR_ID => self.vendor_id,
            VIRTIO_MMIO_STATUS => self.status,
            VIRTIO_MMIO_INTERRUPT_STATUS => self.interrupt_state | VIRTIO_MMIO_INT_VRING,
            VIRTIO_MMIO_QUEUE_NUM_MAX => vq.size_max,
            VIRTIO_MMIO_DEVICE_FEATURES => {
                if self.device_features_sel > 1 {
                    return Err(Error::InvalidFeatureSel(self.device_features_sel));
                }

                let mut features = gdev.device_features();
                features |= 1 << VIRTIO_F_VERSION_1;
                features |= 1 << VIRTIO_F_IOMMU_PLATFORM;
                (features >> (32 * self.device_features_sel)) as u32
            }
            VIRTIO_MMIO_QUEUE_READY => vq.ready,
            VIRTIO_MMIO_QUEUE_DESC_LOW => vq.desc_lo,
            VIRTIO_MMIO_QUEUE_DESC_HIGH => vq.desc_hi,
            VIRTIO_MMIO_QUEUE_USED_LOW => vq.used_lo,
            VIRTIO_MMIO_QUEUE_USED_HIGH => vq.used_hi,
            VIRTIO_MMIO_QUEUE_AVAIL_LOW => vq.avail_lo,
            VIRTIO_MMIO_QUEUE_AVAIL_HIGH => vq.avail_hi,
            VIRTIO_MMIO_CONFIG_GENERATION => {
                // TODO
                // Reading from this register returns a value describing a version of the device-specific configuration space layout.
                // The driver can then access the configuration space and, when finished, read ConfigGeneration again.
                // If no part of the configuration space has changed between these two ConfigGeneration reads, the returned
                // values are identical. If the values are different, the configuration space accesses were not atomic and the
                // driver has to perform the operations again.
                // More info: https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html#x1-1650002
                //            https://docs.oasis-open.org/virtio/virtio/v1.2/csd01/virtio-v1.2-csd01.html#x1-220005
                0
            }
            _ => return Err(Error::InvalidMmioAddr("read", offset)),
        } as u64;

        Ok(())
    }

    /// Method to perform an I/O write operation.
    ///
    /// # Arguments
    ///
    /// * `req` - BaoIoRequest object.
    /// * `dev` - BaoDevice object.
    /// * `offset` - Offset of the I/0 access.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn io_write(&mut self, req: &mut BaoIoRequest, dev: &BaoDevice, offset: u64) -> Result<()> {
        // Get the virtqueue.
        let vq = &mut self.vq[self.queue_sel as usize];

        // Write the data to the device.
        match offset as u32 {
            VIRTIO_MMIO_DEVICE_FEATURES_SEL => self.device_features_sel = req.value as u32,
            VIRTIO_MMIO_DRIVER_FEATURES_SEL => self.driver_features_sel = req.value as u32,
            VIRTIO_MMIO_QUEUE_SEL => self.queue_sel = req.value as u32,
            VIRTIO_MMIO_STATUS => self.status = req.value as u32,
            VIRTIO_MMIO_QUEUE_NUM => vq.size = req.value as u32,
            VIRTIO_MMIO_QUEUE_DESC_LOW => vq.desc_lo = req.value as u32,
            VIRTIO_MMIO_QUEUE_DESC_HIGH => vq.desc_hi = req.value as u32,
            VIRTIO_MMIO_QUEUE_USED_LOW => vq.used_lo = req.value as u32,
            VIRTIO_MMIO_QUEUE_USED_HIGH => vq.used_hi = req.value as u32,
            VIRTIO_MMIO_QUEUE_AVAIL_LOW => vq.avail_lo = req.value as u32,
            VIRTIO_MMIO_QUEUE_AVAIL_HIGH => vq.avail_hi = req.value as u32,
            VIRTIO_MMIO_INTERRUPT_ACK => {
                self.interrupt_state &= !(req.value as u32);
            }
            VIRTIO_MMIO_DRIVER_FEATURES => {
                self.driver_features |=
                    ((req.value as u32) as u64) << (32 * self.driver_features_sel);

                if self.driver_features_sel == 1 {
                    if (self.driver_features & (1 << VIRTIO_F_VERSION_1)) == 0 {
                        return Err(Error::MmioLegacyNotSupported);
                    }
                    if (self.driver_features & (1 << VIRTIO_F_IOMMU_PLATFORM)) == 0 {
                        return Err(Error::IommuPlatformNotSupported);
                    }
                } else {
                    // Guest sends feature sel 1 first, followed by 0. Once that is done, lets
                    // negotiate the vhost-user protocol features.
                    // Note: For now, we don't assume any vhost-user protocol features. We just negotiate the
                    // features that the virtio driver supports. However, by default, the vhost-user frontend
                    // enables: 1) Multiple queues, 2) VirtIO device configuration, 3) Sending reply messages
                    // for requests.
                    dev.gdev
                        .lock()
                        .unwrap()
                        .negotiate_features(
                            self.driver_features,
                            VhostUserProtocolFeatures::empty(),
                        )
                        .map_err(Error::VhostFrontendError)?;
                }
            }
            VIRTIO_MMIO_QUEUE_READY => {
                if req.value == 1 {
                    // Initialize the virtqueue.
                    self.init_vq()?;

                    // Wait for all virtqueues to get initialized.
                    if self.queues.len() == self.queues_count {
                        self.activate_device(dev)?;
                    }
                } else {
                    self.destroy_vq();
                }
            }
            VIRTIO_MMIO_QUEUE_NOTIFY => {
                // This is handled in the Linux kernel now. Nothing to do here.
            }

            _ => return Err(Error::InvalidMmioAddr("write", offset)),
        }

        Ok(())
    }

    /// Method to map a region.
    ///
    /// # Arguments
    ///
    /// * `addr` - Base address to map the region.
    /// * `path` - Path to the file.
    /// * `offset` - Offset of the file.
    /// * `size` - Size of the region.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn map_region(
        &mut self,
        addr: GuestAddress,
        path: &str,
        offset: u64,
        size: usize,
    ) -> Result<()> {
        // Open the file.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        // Create a mmap region with proper permissions.
        let mmap_region = match MmapRegion::build(
            Some(FileOffset::new(file, 0)),
            offset as usize + size as usize,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
        ) {
            Ok(mmap_region) => mmap_region,
            Err(_) => {
                return Err(Error::MmapGuestMemoryFailed);
            }
        };

        // Create a guest region mmap.
        let guest_region_mmap = match GuestRegionMmap::new(mmap_region, addr) {
            Ok(guest_region_mmap) => guest_region_mmap,
            Err(_) => {
                return Err(Error::MmapGuestMemoryFailed);
            }
        };

        // Push the region to the regions vector.
        // For now, we only have one region since this function is called only once.
        // However, in the future, we may have to support more than one region.
        self.regions.push(guest_region_mmap);

        // Return Ok.
        Ok(())
    }

    /// Method to initialize the virtqueues.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn init_vq(&mut self) -> Result<()> {
        let vq = &mut self.vq[self.queue_sel as usize];
        let kick = vq.kick.try_clone().unwrap();
        let vq_size = vq.size;

        // Get the virtqueue addresses.
        let desc = (((vq.desc_hi as u64) << 32) | vq.desc_lo as u64) as u64;
        let avail = (((vq.avail_hi as u64) << 32) | vq.avail_lo as u64) as u64;
        let used = (((vq.used_hi as u64) << 32) | vq.used_lo as u64) as u64;

        let mut queue = Queue::new(vq_size as u16).unwrap();
        queue.set_desc_table_address(Some((desc & 0xFFFFFFFF) as u32), Some((desc >> 32) as u32));
        queue.set_avail_ring_address(
            Some((avail & 0xFFFFFFFF) as u32),
            Some((avail >> 32) as u32),
        );
        queue.set_used_ring_address(Some((used & 0xFFFFFFFF) as u32), Some((used >> 32) as u32));
        queue.set_next_avail(0);

        vq.ready = 1;

        self.queues.push((self.queue_sel as usize, queue, kick));

        Ok(())
    }

    /// Method to destroy the virtqueues.
    fn destroy_vq(&mut self) {
        self.queues.drain(..);
    }

    /// Method to get the memory of the device.
    ///
    /// # Returns
    ///
    /// * `GuestMemoryAtomic<GuestMemoryMmap>` - Guest memory mmap.
    fn mem(&mut self) -> GuestMemoryAtomic<GuestMemoryMmap> {
        GuestMemoryAtomic::new(
            GuestMemoryMmap::from_regions(self.regions.drain(..).collect()).unwrap(),
        )
    }

    /// Method to activate the device.
    ///
    /// # Arguments
    ///
    /// * `dev` - BaoDevice object.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    fn activate_device(&mut self, dev: &BaoDevice) -> Result<()> {
        dev.gdev
            .lock()
            .unwrap()
            .activate(self.mem(), dev.interrupt(), self.queues.drain(..).collect())
            .map_err(Error::VhostFrontendActivateError)
    }

    /// Method to handle an I/O event.
    ///
    /// # Arguments
    ///
    /// * `req` - BaoIoRequest object with the I/O request.
    /// * `dev` - BaoDevice object with the associated device.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn io_event(&mut self, req: &mut BaoIoRequest, dev: &BaoDevice) -> Result<()> {
        let mut offset = req.reg_off;
        if offset >= VHOST_USER_CONFIG_OFFSET as u64 {
            offset -= VHOST_USER_CONFIG_OFFSET as u64;
            let gdev = &mut dev.gdev.lock().unwrap();

            match req.op {
                BAO_IO_READ => self.config_read(req, gdev, offset),
                BAO_IO_WRITE => self.config_write(req, gdev, offset),
                _ => Err(Error::InvalidMmioDir(req.op as u8)),
            }
        } else {
            match req.op {
                BAO_IO_READ => self.io_read(req, dev, offset),
                BAO_IO_WRITE => self.io_write(req, dev, offset),
                _ => Err(Error::InvalidMmioDir(req.op as u8)),
            }
        }
    }
}

impl Drop for BaoMmio {
    /// Destructor function for BaoMmio.
    fn drop(&mut self) {
        for (index, vq) in self.vq.iter().enumerate() {
            let kick = vq.kick.try_clone().unwrap();

            // Create a BaoIoEventFd struct
            let ioeventfd = BaoIoEventFd {
                fd: kick.as_raw_fd() as u32,
                flags: BAO_IOEVENTFD_FLAG_DEASSIGN, // Deassign the eventfd
                addr: self.addr + VIRTIO_MMIO_QUEUE_NOTIFY as u64,
                len: 4,
                reserved: 0,
                data: index as u64, // Index of the Virtqueue to match with the 'value' field of the 'bao_io_request' struct
            };

            // Unregister the kick eventfd.
            self.guest
                .dm
                .lock()
                .unwrap()
                .create_ioeventfd(ioeventfd)
                .unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    // Import the constants from the parent module
    use std::sync::Arc;
    use vm_memory::{Bytes, FileOffset, GuestAddress};
    use vmm_sys_util::tempfile::TempFile;

    // Raw implementation for test purposes
    type GuestMemoryMmap = vm_memory::GuestMemoryMmap<()>;
    type GuestRegionMmap = vm_memory::GuestRegionMmap<()>;
    //type MmapRegion = vm_memory::MmapRegion<()>;

    /// Write and read with offset zero.
    #[test]
    fn write_and_read_offset_zero() {
        // Constants
        const FILE_OFFSET: u64 = 0x0;
        const FILE_SIZE: u64 = 0x400;
        const GUEST_ADDR_INIT: u64 = 0x1000;
        const GUEST_ADDR_END: u64 = 0x13ff;

        // Create a new temp file
        let f = TempFile::new().unwrap().into_file();
        // Set the length of the file
        f.set_len(FILE_SIZE).unwrap();

        // Get a mutable reference to guest address
        let mut start_addr = GuestAddress(GUEST_ADDR_INIT);

        // Create a new GuestMemoryMmap
        let gm = GuestMemoryMmap::from_ranges(&[(start_addr, FILE_SIZE as usize)]).unwrap();

        // Create a new GuestMemoryMmap backed by a file
        let gm_backed_by_file = GuestMemoryMmap::from_ranges_with_files(&[(
            start_addr,
            FILE_SIZE as usize,
            Some(FileOffset::new(f, FILE_OFFSET)),
        )])
        .unwrap();

        // Create a new vector of GuestMemoryMmap
        let gm_list = vec![gm, gm_backed_by_file];

        // Iterate over the vector of GuestMemoryMmap
        for gm in gm_list.iter() {
            // Create a new buffer to write to the guest memory
            let sample_buf = &[1, 2, 3, 4, 5];

            // Write the buffer to the guest memory and assert the number of bytes written
            assert_eq!(gm.write(sample_buf, start_addr).unwrap(), 5);

            // Create a new buffer to read from the guest memory
            let buf = &mut [0u8; 5];

            // Read the buffer from the guest memory and assert the number of bytes read
            assert_eq!(gm.read(buf, start_addr).unwrap(), 5);

            // Assert the buffers are equal
            assert_eq!(buf, sample_buf);

            // Update the start address to the last address of the guest memory
            start_addr = GuestAddress(GUEST_ADDR_END);

            // Write the buffer to the guest memory and assert the number of bytes written
            assert_eq!(gm.write(sample_buf, start_addr).unwrap(), 1);
            // Read the buffer from the guest memory and assert the number of bytes read
            assert_eq!(gm.read(buf, start_addr).unwrap(), 1);
            // Assert the buffers are equal
            assert_eq!(buf[0], sample_buf[0]);
            // Update the start address to the first address of the guest memory
            start_addr = GuestAddress(GUEST_ADDR_INIT);
        }
    }

    /// Write and read with offset greater than zero.
    #[test]
    fn write_and_read_offset_greater_than_zero() {
        // Constants
        const FILE_OFFSET: u64 = 0x1000;
        const FILE_SIZE: u64 = 0x400;
        const GUEST_ADDR_INIT: u64 = 0x0;
        const GUEST_ADDR_END: u64 = 0x3ff;

        // Create a new temp file
        let f = TempFile::new().unwrap().into_file();
        // Set the length of the file
        f.set_len(FILE_OFFSET + FILE_SIZE).unwrap();

        // Get a mutable reference to guest address
        let mut start_addr = GuestAddress(GUEST_ADDR_INIT);

        // Create a new GuestMemoryMmap
        let gm = GuestMemoryMmap::from_ranges(&[(start_addr, FILE_SIZE as usize)]).unwrap();

        // Create a new GuestMemoryMmap backed by a file
        let gm_backed_by_file = GuestMemoryMmap::from_ranges_with_files(&[(
            start_addr,
            FILE_SIZE as usize,
            Some(FileOffset::new(f, FILE_OFFSET)),
        )])
        .unwrap();

        // Create a new vector of GuestMemoryMmap
        let gm_list = vec![gm, gm_backed_by_file];

        // Iterate over the vector of GuestMemoryMmap
        for gm in gm_list.iter() {
            // Create a new buffer to write to the guest memory
            let sample_buf = &[1, 2, 3, 4, 5];

            // Write the buffer to the guest memory and assert the number of bytes written
            assert_eq!(gm.write(sample_buf, start_addr).unwrap(), 5);

            // Create a new buffer to read from the guest memory
            let buf = &mut [0u8; 5];

            // Read the buffer from the guest memory and assert the number of bytes read
            assert_eq!(gm.read(buf, start_addr).unwrap(), 5);

            // Assert the buffers are equal
            assert_eq!(buf, sample_buf);

            // Update the start address to the last address of the guest memory
            start_addr = GuestAddress(GUEST_ADDR_END);

            // Write the buffer to the guest memory and assert the number of bytes written
            assert_eq!(gm.write(sample_buf, start_addr).unwrap(), 1);
            // Read the buffer from the guest memory and assert the number of bytes read
            assert_eq!(gm.read(buf, start_addr).unwrap(), 1);
            // Assert the buffers are equal
            assert_eq!(buf[0], sample_buf[0]);
            // Update the start address to the first address of the guest memory
            start_addr = GuestAddress(GUEST_ADDR_INIT);
        }
    }

    /// Write and read within a region.
    #[test]
    fn write_and_read_region_mmap() {
        // Constants
        const FILE_OFFSET: u64 = 0x1000;
        const FILE_SIZE: u64 = 0x400;
        const GUEST_ADDR_INIT: u64 = 0x0;
        const GUEST_ADDR_END: u64 = 0x3ff;

        // Create a new temp file
        let f = TempFile::new().unwrap().into_file();
        // Set the length of the file
        f.set_len(FILE_OFFSET + FILE_SIZE).unwrap();
        assert_eq!(f.metadata().unwrap().len(), FILE_OFFSET + FILE_SIZE);
        // Clone the file
        let f_clone = f.try_clone().unwrap();
        // Wrap the file with a Arc
        let file = Arc::new(f);

        // Get a mutable reference to guest address
        let mut start_addr = GuestAddress(GUEST_ADDR_INIT);

        // Create the region
        let mut regions = Vec::new();
        let region = GuestRegionMmap::from_range(
            start_addr,
            FILE_SIZE as usize,
            Some(FileOffset::from_arc(file, FILE_OFFSET)),
        )
        .unwrap();

        // Push the region to the regions vector
        regions.push(region);

        // Create a new GuestMemoryMmap with the regions vector
        let gm = GuestMemoryMmap::from_regions(regions).unwrap();

        // Create a new GuestMemoryMmap backed by a file
        let gm_backed_by_file = GuestMemoryMmap::from_ranges_with_files(&[(
            start_addr,
            FILE_SIZE as usize,
            Some(FileOffset::new(f_clone, FILE_OFFSET)),
        )])
        .unwrap();

        // Create a new vector of GuestMemoryMmap
        let gm_list = vec![gm, gm_backed_by_file];

        // Iterate over the vector of GuestMemoryMmap
        for gm in gm_list.iter() {
            // Create a new buffer to write to the guest memory
            let sample_buf = &[1, 2, 3, 4, 5];

            // Write the buffer to the guest memory and assert the number of bytes written
            assert_eq!(gm.write(sample_buf, start_addr).unwrap(), 5);

            // Create a new buffer to read from the guest memory
            let buf = &mut [0u8; 5];

            // Read the buffer from the guest memory and assert the number of bytes read
            assert_eq!(gm.read(buf, start_addr).unwrap(), 5);

            // Assert the buffers are equal
            assert_eq!(buf, sample_buf);

            // Update the start address to the last address of the guest memory
            start_addr = GuestAddress(GUEST_ADDR_END);

            // Write the buffer to the guest memory and assert the number of bytes written
            assert_eq!(gm.write(sample_buf, start_addr).unwrap(), 1);
            // Read the buffer from the guest memory and assert the number of bytes read
            assert_eq!(gm.read(buf, start_addr).unwrap(), 1);
            // Assert the buffers are equal
            assert_eq!(buf[0], sample_buf[0]);
            // Update the start address to the first address of the guest memory
            start_addr = GuestAddress(GUEST_ADDR_INIT);
        }
    }
}
