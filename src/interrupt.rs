// Copyright (c) Bao Project and Contributors. All rights reserved.
//          João Peixoto <joaopeixotooficial@gmail.com>
//
// SPDX-License-Identifier: Apache-2.0

//! The 'Interrupt' module serves as an abstraction to implement device interrupts
//! functionalities in form of Irqfds.

use super::device::BaoDevice;
use bao_sys::{defines::*, error::*, types::*};
use std::os::fd::AsRawFd;
use std::{io::Result as IoResult, sync::Arc};
use vhost_user_frontend::{VirtioInterrupt, VirtioInterruptType};
use vmm_sys_util::eventfd::EventFd;

/// Struct representing a BAO VirtIO interrupt
///
/// # Attributes
///
/// * `dev` - The BaoDevice associated with the interrupt.
/// * `call` - The EventFd associated with the interrupt.
pub struct BaoInterrupt {
    dev: Arc<BaoDevice>,
    call: EventFd,
}

impl BaoInterrupt {
    /// Constructor function for BaoInterrupt.
    ///
    /// # Arguments
    ///
    /// * `dev` - The BaoDevice associated with the interrupt.
    ///
    /// # Return
    ///
    /// * `Result<Arc<Self>>` - A Result containing an Arc of the BaoInterrupt.
    pub fn new(dev: Arc<BaoDevice>) -> Result<Arc<Self>> {
        // Create a new EventFd for the interrupt
        let call = EventFd::new(0).unwrap();

        // Create a new BaoInterrupt
        let bao_int = Arc::new(BaoInterrupt {
            dev,
            call: call.try_clone().unwrap(),
        });

        // Create a BaoIrqFd struct
        let irqfd = BaoIrqFd {
            fd: bao_int.call.as_raw_fd() as i32,
            flags: BAO_IRQFD_FLAG_ASSIGN, // Assign the Irqfd
        };

        // Create an Irqdf for the interrupt
        match bao_int.dev.guest.dm.lock().unwrap().create_irqfd(irqfd) {
            Ok(_) => (),
            Err(err) => return Err(err),
        }

        // Return the BaoInterrupt
        Ok(bao_int)
    }

    /// Method to exit the BaoInterrupt.
    ///
    /// # Return
    ///
    /// * `Result<()>` - A Result containing Ok(()) on success, or an Error on failure.
    pub fn exit(&self) -> Result<()> {
        // Create a BaoIrqFd struct
        let irqfd = BaoIrqFd {
            fd: self.call.as_raw_fd() as i32,
            flags: BAO_IRQFD_FLAG_DEASSIGN, // Deassign the Irqfd
        };

        // Destroy the Irqfd for the interrupt
        match self.dev.guest.dm.lock().unwrap().create_irqfd(irqfd) {
            Ok(_) => (),
            Err(err) => return Err(err),
        }

        // Return Ok if everything went well
        Ok(())
    }
}

impl VirtioInterrupt for BaoInterrupt {
    /// Implementation of the trigger method of the VirtioInterrupt trait for BaoInterrupt.
    ///
    /// # Arguments
    ///
    /// * `_int_type` - The type of the interrupt (Used Buffer or Configuration Change Notification).
    ///
    /// # Return
    ///
    /// * `IoResult<()>` - An IoResult containing Ok(()) on success, or an Error on failure.
    fn trigger(&self, _int_type: VirtioInterruptType) -> IoResult<()> {
        Ok(())
    }

    /// Implementation of the notifier method of the VirtioInterrupt trait for BaoInterrupt.
    ///
    /// # Arguments
    ///
    /// * `_int_type` - The type of the interrupt (Used Buffer or Configuration Change Notification).
    ///
    /// # Return
    ///
    /// * `Option<EventFd>` - An Option containing the EventFd associated with the interrupt.
    fn notifier(&self, _int_type: VirtioInterruptType) -> Option<EventFd> {
        Some(self.call.try_clone().unwrap())
    }
}
