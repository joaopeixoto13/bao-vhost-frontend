// Copyright (c) Bao Project and Contributors. All rights reserved.
//          João Peixoto <joaopeixotooficial@gmail.com>
//
// SPDX-License-Identifier: Apache-2.0

//! The 'Frontend' module encapsulates the master side of the vhost-user
//! protocol and specifies a Bao Hypervisor vhost-user frontend implementation.
//! This abstraction facilitates seamless interaction with guests and devices,
//! offering essential functionalities like adding or removing guests and managing devices.
//!
//! # Architecture
//!                 
//! Frontend 1
//!
//!
//! ├── Guest 1.1
//!
//!     ├── Device 1.1.1
//!
//!     └── Device 1.1.2
//!
//!
//! └── Guest 1.2
//!
//!     ├── Device 1.2.1
//!
//!     └── Device 1.2.2
//!
//! Frontend 2
//!
//!
//! ├── Guest 2.1
//!
//!     ├── Device 2.1.1
//!
//!     └── Device 2.1.2
//!
//!
//! └── Guest 2.2
//!
//!     ├── Device 2.2.1
//!
//!     └── Device 2.2.2
//!

use super::{device::BaoDevice, guest::BaoGuest};
use bao_sys::error::*;
use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

/// Represents a collection of BaoGuests.
#[derive(Default)]
struct FrontendGuests(Vec<Arc<BaoGuest>>);

impl FrontendGuests {
    /// Finds a guest with the given Guest ID.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest to be found.
    ///
    /// # Returns
    ///
    /// * `Option<Arc<BaoGuest>>` - A cloned Arc to the found guest or None if not found.
    fn find(&self, guest_id: u16) -> Option<Arc<BaoGuest>> {
        // Searches for a guest with the provided Guest ID in the internal vector.
        // Returns a cloned Arc to the found guest or None if not found.
        self.0.iter().find(|guest| guest.id == guest_id).cloned()
    }

    /// Adds a new guest with the provided Guest ID to the collection.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest to be added.
    /// * `ram_addr` - The RAM base address of the guest to be added.
    /// * `ram_size` - The RAM size of the guest to be added.
    ///
    /// # Returns
    ///
    /// * `Result<Arc<BaoGuest>>` - A cloned Arc to the newly created guest as a Result.
    fn add(&mut self, guest_id: u16, ram_addr: u64, ram_size: u64) -> Result<Arc<BaoGuest>> {
        // Creates a new BaoGuest with the given Guest ID.
        let guest = BaoGuest::new(guest_id, ram_addr, ram_size)?;

        // Clones the Arc of the new guest and appends it to the internal vector.
        self.0.push(guest.clone());

        // Returns the cloned guest as a Result.
        Ok(guest)
    }

    /// Removes a guest with the given Guest ID from the collection.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest to be removed.
    fn remove(&mut self, guest_id: u16) {
        // Finds the position of the guest with the provided Guest ID in the internal vector
        // and removes it from the vector, then calls its `exit()` method.
        self.0
            .remove(
                self.0
                    .iter()
                    .position(|guest| guest.id == guest_id)
                    .unwrap(),
            )
            .exit()
    }

    /// Adds a device to the guest.
    /// If the guest does not exist, creates a new guest and adds the device to it.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest to which the device will be added.
    /// * `dev_id` - The Device ID of the device to be added.
    /// * `dev_irq` - The Device IRQ of the device to be added.
    /// * `dev_addr` - The Device address of the device to be added.
    /// * `ram_addr` - The RAM base address of the guest to which the device will be added.
    /// * `ram_size` - The RAM size of the guest to which the device will be added.
    /// * `shmem_path` - The shared memory path of the guest to which the device will be added.
    /// * `socket_path` - The socket path of the guest to which the device will be added.
    ///
    /// # Returns
    ///
    /// * `Result<Arc<BaoDevice>>` - A cloned Arc to the newly created device as a Result.
    fn add_device(
        &mut self,
        guest_id: u16,
        dev_id: u64,
        dev_irq: u64,
        dev_addr: u64,
        ram_addr: u64,
        ram_size: u64,
        shmem_path: String,
        socket_path: String,
    ) -> Result<Arc<BaoDevice>> {
        // Attempts to find the guest with the provided Guest ID.
        // If found, adds the device to that guest; otherwise, creates a new guest and adds the device.
        let guest = match self.find(guest_id) {
            Some(guest) => guest,
            None => self.add(guest_id, ram_addr, ram_size)?,
        };

        // Delegates the addition of the device to the found or newly created guest.
        guest.add_device(
            dev_id,
            dev_irq,
            dev_addr,
            ram_addr,
            ram_size,
            shmem_path,
            socket_path,
        )
    }

    /// Removes a device from the guest with the given Guest ID.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest from which the device will be removed.
    /// * `dev_addr` - The Device address of the device to be removed.
    fn remove_device(&mut self, guest_id: u16, dev_addr: u64) {
        // Finds the guest with the provided Guest ID.
        let guest = self.find(guest_id).unwrap();

        // Removes the device with the provided device ID from the guest.
        guest.remove_device(dev_addr);

        // Checks if the guest is empty after device removal and removes the guest if so.
        if guest.is_empty() {
            self.remove(guest_id);
        }
    }
}

/// Represents a Bao Frontend.
///
/// # Attributes
///
/// * `guests` - The guests of the frontend.
/// * `threads` - The threads of the frontend.
pub struct BaoFrontend {
    guests: Mutex<FrontendGuests>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl BaoFrontend {
    /// Creates a new instance of BaoFrontend wrapped in an Arc.
    pub fn new() -> Result<Arc<Self>> {
        // Creates a new instance of BaoFrontend wrapped in an Arc
        Ok(Arc::new(Self {
            guests: Mutex::new(FrontendGuests::default()), // Initializes FrontendGuests with default values and wraps it in a Mutex
            threads: Mutex::new(Vec::new()), // Initializes an empty Vec and wraps it in a Mutex
        }))
    }

    /// Adds a device to the Frontend.
    /// If the guest does not exist, creates a new guest and adds the device to it.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest to which the device will be added.
    /// * `dev_id` - The Device ID of the device to be added.
    /// * `dev_irq` - The Device IRQ of the device to be added.
    /// * `dev_addr` - The Device address of the device to be added.
    /// * `ram_addr` - The RAM base address of the guest to which the device will be added.
    /// * `ram_size` - The RAM size of the guest to which the device will be added.
    /// * `shmem_path` - The shared memory path of the guest to which the device will be added.
    /// * `socket_path` - The socket path of the guest to which the device will be added.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Ok if the device was added successfully, otherwise an error.
    ///
    /// # Examples
    ///
    /// ```
    /// const GUEST_ID: u16 = 0;
    /// const DEV_ID: u64 = 4; // rng
    /// const DEV_IRQ: u64 = 0x2f;
    /// const DEV_ADDR: u64 = 0xa003e00;
    /// const RAM_ADDR: u64 = 0x60000000;
    /// const RAM_SIZE: u64 = 0x01000000;
    /// const SHMEM_PATH: String = String::from("/dev/baoipc0");
    /// const SOCKET_PATH: String = String::from("/root/");
    ///
    /// let frontend = BaoFrontend::new().unwrap();
    /// let fe: std::sync::Arc<BaoFrontend> = frontend.clone();
    /// fe.add_device(GUEST_ID, DEV_ID, DEV_IRQ, DEV_ADDR, RAM_ADDR, RAM_SIZE, SHMEM_PATH, SOCKET_PATH).unwrap();
    /// ```
    pub fn add_device(
        &self,
        guest_id: u16,
        dev_id: u64,
        dev_irq: u64,
        dev_addr: u64,
        ram_addr: u64,
        ram_size: u64,
        shmem_path: String,
        socket_path: String,
    ) -> Result<()> {
        // Adds a device for the given guest_id and dev_id to the guests using a Mutex lock
        let dev = self.guests.lock().unwrap().add_device(
            guest_id,
            dev_id,
            dev_irq,
            dev_addr,
            ram_addr,
            ram_size,
            shmem_path,
            socket_path,
        )?;

        // Enable the guest to receive I/O events
        dev.guest.enable_io_events();

        // Returns Ok
        Ok(())
    }

    /// Removes a device from the Frontend with the given Guest ID and device ID.
    ///
    /// # Arguments
    ///
    /// * `guest_id` - The Guest ID of the guest from which the device will be removed.
    /// * `dev_addr` - The Device address of the device to be removed.
    ///
    /// # Examples
    ///
    /// ```
    /// const GUEST_ID: u16 = 0;
    /// const DEV_ADDR: u64 = 0xa003e00;
    ///
    /// let frontend = BaoFrontend::new().unwrap();
    /// let fe: std::sync::Arc<BaoFrontend> = frontend.clone();
    /// fe.remove_device(GUEST_ID, DEV_ADDR);
    /// ```
    pub fn remove_device(&self, guest_id: u16, dev_addr: u64) {
        // Removes a device for the given fe_id and dev_id from the guests using a Mutex lock
        self.guests
            .lock()
            .unwrap()
            .remove_device(guest_id, dev_addr);
    }

    /// Pushes a JoinHandle to the Frontend threads.
    ///
    /// # Arguments
    ///
    /// * `handle` - The JoinHandle to be pushed.
    ///
    /// # Examples
    ///
    /// ```
    /// const GUEST_ID: u16 = 0;
    /// const DEV_ID: u64 = 4; // rng
    /// const DEV_IRQ: u64 = 0x2f;
    /// const DEV_ADDR: u64 = 0xa003e00;
    /// const RAM_ADDR: u64 = 0x60000000;
    /// const RAM_SIZE: u64 = 0x01000000;
    ///
    /// let frontend = BaoFrontend::new().unwrap();
    /// let fe: std::sync::Arc<BaoFrontend> = frontend.clone();
    ///
    /// fe.push_thread(
    ///     Builder::new()
    ///         .name(format!("frontend {} - {}", fe_id, dev_id))
    ///         .spawn(move || {
    ///             match fe.add_device(GUEST_ID, DEV_ID, DEV_IRQ, DEV_ADDR, RAM_ADDR, RAM_SIZE) {
    ///                 Ok(_) => { }
    ///                 Err(err) => { fe.remove_device(GUEST_ID, DEV_ADDR); }
    ///             }
    ///         })
    ///         .unwrap(),
    /// );
    /// ```
    pub fn push_thread(&self, handle: JoinHandle<()>) {
        // Pushes a JoinHandle to the threads using a Mutex lock
        self.threads.lock().unwrap().push(handle)
    }
}

impl Drop for BaoFrontend {
    /// Drops all handles from the threads vector.
    fn drop(&mut self) {
        // Loops until all handles are popped from the threads vector
        while let Some(handle) = self.threads.lock().unwrap().pop() {
            // Joins the thread represented by the handle
            handle.join().unwrap();
        }
    }
}
