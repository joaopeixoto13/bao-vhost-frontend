// Copyright (c) Bao Project and Contributors. All rights reserved.
//          João Peixoto <joaopeixotooficial@gmail.com>
//
// SPDX-License-Identifier: Apache-2.0

//! The 'Device' module serves as the manager for I/O device operations, encapsulating
//! essential attributes and functionalities to handle I/O interactions effectively.

use clap::Parser;
use seccompiler::SeccompAction;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use lazy_static::lazy_static;
use vhost_user_frontend::{Generic, VhostUserConfig, VirtioDevice, VirtioDeviceType};
use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use super::{guest::BaoGuest, interrupt::BaoInterrupt, mmio::BaoMmio};
use bao_sys::{defines::*, error::*, types::*};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
/// Device arguments
///
/// # Attributes
///
/// * `socket_path` - Location of vhost-user Unix domain socket.
struct DeviceArgs {
    #[clap(short, long)] // Attributes indicating it accepts both short and long arguments
    socket_path: String,
}

/// Define information
///
/// # Attributes
///
/// * `name` - The name of the device.
/// * `compatible` - The compatible string of the device.
/// * `index` - The index of the device.
struct DeviceInfo {
    name: &'static str,
    compatible: String,
    index: u32,
}

impl DeviceInfo {
    /// Constructor function for DeviceInfo.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the device.
    /// * `id` - The id of the device.
    ///
    /// # Return
    ///
    /// * `DeviceInfo` - A DeviceInfo object.
    fn new(name: &'static str, id: u32) -> Self {
        DeviceInfo {
            // Initialize name field with provided value
            name,
            // Generate the compatible string based on the id
            compatible: format!("virtio,device{}", id),
            // Initialize index to 0
            index: 0,
        }
    }

    /// Method to increment and return the index as a string.
    /// This method is used to be possible to have multiple devices of the same type
    /// within the same guest.
    ///
    /// # Return
    ///
    /// * `String` - The index as a string.
    fn index(&mut self) -> String {
        // Increment the index
        self.index += 1;
        // Return the index as a string
        (self.index - 1).to_string()
    }
}

lazy_static! {
    /// Devices HashMap (Mutex protected)
    static ref DEVICES: Mutex<HashMap<String, DeviceInfo>> = {
        let mut map = HashMap::new();

        // Iterate over the supported devices
        for entry in SUPPORTED_DEVICES.iter() {
            // Create a new DeviceInfo for each supported device
            let dev = DeviceInfo::new(entry.0, entry.1);
            // Insert the device into the HashMap
            map.insert(dev.compatible.clone(), dev);
        }
        // Wrap the HashMap in a Mutex
        Mutex::new(map)
    };
}

/// Bao Device.
///
/// # Attributes
///
/// * `gdev` - The Generic vhost-user device.
/// * `mmio` - The BaoMmio device.
/// * `id` - The id of the device.
/// * `irq` - The irq of the device.
/// * `addr` - The address of the device.
/// * `guest` - The guest that owns the device.
/// * `interrupt` - The interrupt of the device.
pub struct BaoDevice {
    pub gdev: Mutex<Generic>,
    pub mmio: Mutex<BaoMmio>,
    pub id: u64,
    pub irq: u64,
    pub addr: u64,
    pub guest: Arc<BaoGuest>,
    interrupt: Mutex<Option<Arc<BaoInterrupt>>>,
}

impl BaoDevice {
    /// Constructor function for BaoDevice.
    ///
    /// # Arguments
    ///
    /// * `id` - The id of the device.
    /// * `irq` - The irq of the device.
    /// * `addr` - The address of the device.
    /// * `ram_addr` - The address of the guest RAM.
    /// * `ram_size` - The size of the guest RAM.
    /// * `socket_path` - The path to the vhost-user socket.
    /// * `guest` - The guest that owns the device.
    ///
    /// # Return
    ///
    /// * `Result<Arc<Self>>` - A Result object containing the BaoDevice.
    pub fn new(
        id: u64,
        irq: u64,
        addr: u64,
        ram_addr: u64,
        ram_size: u64,
        shmem_path: String,
        socket_path: String,
        guest: Arc<BaoGuest>,
    ) -> Result<Arc<Self>> {
        // Extract the supported devices HashMap
        let mut devices = DEVICES.lock().unwrap();

        // Generate the compatible string based on the device id
        let compatible = format!("virtio,device{}", id);

        // Extract the device based on the key (compatible string)
        let dev = devices
            .get_mut(&compatible)
            .ok_or(Error::BaoDevNotSupported(compatible))?;

        // Extract the device type
        let device_type = VirtioDeviceType::from(dev.name);

        // Extract the number of queues and queue size
        let (num, size) = device_type.queue_num_and_size();

        // Create the vhost-user configuration
        let vu_cfg = VhostUserConfig {
            socket: socket_path + dev.name + ".sock" + &dev.index(),
            num_queues: num,
            queue_size: size as u16,
        };

        println!(
            "Connecting to {} device backend over {} socket..",
            dev.name, vu_cfg.socket
        );

        // Create the Generic vhost-user device
        let gdev = Generic::new(
            vu_cfg,
            SeccompAction::Allow,
            EventFd::new(EFD_NONBLOCK).unwrap(),
            device_type,
        )
        .map_err(Error::VhostFrontendError)?;

        println!("Connected to {} device backend.", dev.name);

        // Create the BaoMmio device
        let mmio = match BaoMmio::new(&gdev, guest.clone(), addr, ram_addr, ram_size, shmem_path) {
            Ok(mmio) => mmio,
            Err(err) => return Err(err),
        };

        // Create the BaoDevice
        let dev = Arc::new(Self {
            gdev: Mutex::new(gdev),
            mmio: Mutex::new(mmio),
            id,
            irq,
            addr,
            guest,
            interrupt: Mutex::new(None),
        });

        // Create the BaoInterrupt
        match BaoInterrupt::new(dev.clone()) {
            Ok(int) => {
                // Store the interrupt
                *dev.interrupt.lock().unwrap() = Some(int);
            }
            Err(err) => return Err(err),
        }

        // Return the BaoDevice
        Ok(dev)
    }

    /// Interrupt getter.
    ///
    /// # Return
    ///
    /// * `Arc<BaoInterrupt>` - A Bao interrupt object.
    pub fn interrupt(&self) -> Arc<BaoInterrupt> {
        // We use interrupt.take() here to drop the reference to Arc<BaoInterrupt>, as the same
        // isn't required anymore.
        self.interrupt.lock().unwrap().as_ref().unwrap().clone()
    }
    /// Handles I/O events for the BaoDevice based on the given request.
    ///
    /// # Arguments
    ///
    /// * `req` - The BaoIoRequest to be handled.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn io_event(&self, req: &mut BaoIoRequest) -> Result<()> {
        // Call the io_event method of the BaoMmio device
        self.mmio.lock().unwrap().io_event(req, self)
    }

    /// Method to exit/deactivate the BaoDevice.
    pub fn exit(&self) {
        if let Some(interrupt) = self.interrupt.lock().unwrap().take() {
            interrupt.exit().unwrap();
        }
        // Deactivate the device
        self.gdev.lock().unwrap().reset();
        // Shutdown the device
        self.gdev.lock().unwrap().shutdown();
    }
}
