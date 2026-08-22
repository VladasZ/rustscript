#!/usr/bin/env rust

// Read only, so always safe to run. `Windows` only.

use windows_service::service::ServiceAccess;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

fn main() {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .expect("open the service manager");

    // `w32time` exists on every `Windows` install
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG;
    let svc = manager
        .open_service("w32time", access)
        .expect("open w32time");

    let status = svc.query_status().expect("read the service status");
    let config = svc.query_config().expect("read the service config");

    println!("state = {:?}", status.current_state);
    println!("start_type = {:?}", config.start_type);

    // a missing service is an error, not a panic
    let missing = manager.open_service("thing_no_such_service", access);
    assert!(missing.is_err());
    println!("missing service reports an error, as expected");
}
