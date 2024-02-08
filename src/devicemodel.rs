// Copyright (c) Bao Project and Contributors. All rights reserved.
//          João Peixoto <joaopeixotooficial@gmail.com>
//
// SPDX-License-Identifier: Apache-2.0

//! The 'Device Model' module contains the Bao Device Model, which is responsible for interacting with the
//! I/O Request Management System inside the kernel via IOCTLs to Bao the device file descriptor `/dev/bao`.

#![allow(dead_code)]

use bao_sys::{defines::*, error::*, ioctl::*, types::*};
use libc::ioctl;
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;

/// Represents a BaoDeviceModel.
///
/// # Attributes
///
/// * `guest_id` - Guest ID.
/// * `fd` - File descriptor for the guest.
/// * `guest_fd` - File descriptor for the guest.
/// * `ram_addr` - Address of the guest's RAM.
/// * `ram_size` - Size of the guest's RAM.
pub struct BaoDeviceModel {
    guest_id: u16,
    fd: i32,
    guest_fd: i32,
    pub ram_addr: u64,
    pub ram_size: u64,
}

impl BaoDeviceModel {
    /// Creates a new BaoDeviceModel.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - Guest ID.
    /// * `ram_addr` - Address of the guest's RAM.
    /// * `ram_size` - Size of the guest's RAM.
    ///
    /// # Return
    ///
    /// * `Result<BaoDeviceModel>` -  A Result object containing the BaoDeviceModel object on success.
    pub fn new(guest_id: u16, ram_addr: u64, ram_size: u64) -> Result<Self> {
        // Open the Bao device file
        let fd = OpenOptions::new().read(true).write(true).open("/dev/bao");

        // Check if the file was opened successfully
        match fd {
            Ok(fd) => {
                let guest_fd: i32;
                // Create a new VM VirtIO backend
                unsafe {
                    guest_fd = ioctl(
                        fd.as_raw_fd(),
                        BAO_IOCTL_VM_VIRTIO_BACKEND_CREATE(),
                        &(guest_id as i32),
                    );

                    if guest_fd < 0 {
                        // close the file
                        File::try_clone(&fd).unwrap();
                        return Err(Error::OpenFdFailed(
                            "guest_fd",
                            std::io::Error::last_os_error(),
                        ));
                    }
                }

                // Create a new BaoDeviceModel
                let dm: BaoDeviceModel = BaoDeviceModel {
                    guest_id,
                    fd: fd.as_raw_fd(),
                    guest_fd,
                    ram_addr,
                    ram_size,
                };
                // Return the new BaoDeviceModel object
                return Ok(dm);
            }
            Err(_) => {
                return Err(Error::OpenFdFailed(
                    "/dev/bao",
                    std::io::Error::last_os_error(),
                ));
            }
        }
    }

    /// Destroys the BaoDeviceModel.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn destroy(&mut self) -> Result<()> {
        // Destroy the VM VirtIO backend
        unsafe {
            let ret = ioctl(
                self.fd,
                BAO_IOCTL_VM_VIRTIO_BACKEND_DESTROY(),
                &(self.guest_id as i32),
            );

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }
        // Close the file
        self.fd = -1;
        self.guest_fd = -1;
        self.ram_addr = 0;
        self.ram_size = 0;

        // Return Ok(()) on success
        Ok(())
    }

    /// Creates a new I/O client.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn create_io_client(&self) -> Result<()> {
        // Create a new I/O client
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IO_CREATE_CLIENT(), &self.guest_fd);

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }

    /// Destroys the I/O client.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn destroy_io_client(&self) -> Result<()> {
        // Destroy the I/O client
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IO_DESTROY_CLIENT());

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }

    /// Attaches the I/O client.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn attach_io_client(&self) -> Result<()> {
        // Attach the I/O client
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IO_ATTACH_CLIENT());

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }

    /// Requests an I/O request.
    ///
    /// # Return
    ///
    /// * `Result<BaoIoRequest>` - A Result containing the BaoIoRequest object on success.
    pub fn request_io(&self) -> Result<BaoIoRequest> {
        // Create a new I/O request
        let mut request = BaoIoRequest {
            virtio_id: 0,
            reg_off: 0,
            addr: 0,
            op: BAO_IO_ASK,
            value: 0,
            access_width: 0,
            ret: 0,
        };
        // Request an I/O request
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IO_REQUEST(), &mut request);

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(request)
    }

    /// Notifies I/O request completion.
    ///
    /// # Arguments
    ///
    /// * `req` - The BaoIoRequest to be notified.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn notify_io_completed(&self, req: BaoIoRequest) -> Result<()> {
        // Notify I/O request completion
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IO_REQUEST_NOTIFY_COMPLETED(), &req);

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }

    /// Notifies the guest about a Used Buffer Notification or
    /// a Configuration Change Notification.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn notify_guest(&self) -> Result<()> {
        // Notify the guest
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IO_NOTIFY_GUEST());

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }

    /// Creates a new ieventfd.
    ///
    /// # Arguments
    ///
    /// * `ev` - The BaoIoEventFd to be created.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn create_ioeventfd(&self, ev: BaoIoEventFd) -> Result<()> {
        // Create a new I/O event file descriptor
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IOEVENTFD(), &ev);

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }

    /// Creates a new irqfd.
    ///
    /// # Arguments
    ///
    /// * `irq` - The BaoIrqFd to be created.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn create_irqfd(&self, irq: BaoIrqFd) -> Result<()> {
        // Create a new IRQ file descriptor
        unsafe {
            let ret = ioctl(self.guest_fd, BAO_IOCTL_IRQFD(), &irq);

            if ret < 0 {
                return Err(Error::BaoIoctlError(
                    std::io::Error::last_os_error(),
                    std::any::type_name::<Self>(),
                ));
            }
        }

        // Return Ok(()) on success
        Ok(())
    }
}
