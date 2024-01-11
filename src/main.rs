mod device;
mod devicemodel;
mod frontend;
mod guest;
mod interrupt;
mod mmio;

use std::thread::Builder;

use bao_sys::utils::parse_arguments;
use frontend::BaoFrontend;

fn main() {
    // Print the starting message
    println!("[Start] bao-vhost-frontend.");

    // Parse the command line arguments
    let config_frontends = parse_arguments().unwrap();

    // Create a new BaoFrontend object
    let frontend = BaoFrontend::new().unwrap();

    // Iterate over frontends
    for config_frontend in config_frontends.frontends.into_iter() {
        // Clone the frontend
        let fe: std::sync::Arc<BaoFrontend> = frontend.clone();
        // Create a new thread for each frontend
        frontend.push_thread(
            Builder::new()
                .name(format!(
                    "frontend {} - {}",
                    config_frontend.name, config_frontend.id
                ))
                .spawn(move || {
                    // Iterate over guests within each frontend
                    for config_guest in config_frontend.guests.iter() {
                        // Iterate over devices within each guest
                        for config_device in config_guest.devices.iter() {
                            match fe.add_device(
                                config_guest.id as u16,
                                config_device.id as u64,
                                config_device.irq as u64,
                                config_device.addr as u64,
                                config_guest.ram_addr,
                                config_guest.ram_size,
                                config_guest.socket_path.clone(),
                            ) {
                                Ok(_) => {
                                    println!(
                                        "Device {} at 0x{:x} added.",
                                        config_device.id, config_device.addr
                                    );
                                }
                                Err(err) => {
                                    println!("Error: {:?}", err);
                                    fe.remove_device(config_guest.id as u16, config_device.addr);
                                }
                            }
                        }
                    }
                })
                .unwrap(),
        );
    }

    // Print the ending message
    println!("[End] bao-vhost-frontend.");

    // Loop forever
    loop {}
}
