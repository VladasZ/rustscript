#!/usr/bin/env rust

// Reads Windows service state through the windows-service bridge. Read only, it
// never starts, stops or reconfigures anything, so running it is always safe.
//
// Windows only. The examples suite skips it elsewhere.

use windows_service::service::ServiceAccess;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

fn main() {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .expect("open the service manager");

    // w32time is the service clock.rs configures, and it exists on every
    // Windows install, so it is a safe thing to look at.
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG;
    let svc = manager
        .open_service("w32time", access)
        .expect("open w32time");

    let status = svc.query_status().expect("read the service status");
    let config = svc.query_config().expect("read the service config");

    println!("state = {:?}", status.current_state);
    println!("start_type = {:?}", config.start_type);

    // A service that is not installed reports an error rather than panicking.
    let missing = manager.open_service("thing_no_such_service", access);
    assert!(missing.is_err());
    println!("missing service reports an error, as expected");
}
