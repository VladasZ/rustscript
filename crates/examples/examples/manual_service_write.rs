#!/usr/bin/env rust

// Exercises the service bridge write paths, which the read only service_demo
// cannot cover. It changes real service state, so it is named manual_ and the
// examples suite never runs it on its own. Run it by hand on a spare box:
//
//   rust crates/examples/examples/manual_service_write.rs
//
// Needs admin. Config changes go against a throwaway service this script
// creates and deletes. Start and stop go against the print spooler, whose
// original state is recorded up front and put back at the end.

use std::process::Command;

use windows_service::service::{ServiceAccess, ServiceInfo, ServiceStartType, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

const TEST_SVC: &str = "thing_rustscript_test";

fn sc(args: Vec<&str>) -> bool {
    Command::new("sc.exe")
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn binpath() -> String {
    let out = Command::new("sc.exe")
        .args(vec!["qc", TEST_SVC])
        .output()
        .expect("sc qc");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for line in text.lines() {
        if line.contains("BINARY_PATH_NAME") {
            return line.trim().to_string();
        }
    }
    String::new()
}

fn main() {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .expect("open the service manager");

    // A dummy binary path is fine here. The service is never started, only
    // reconfigured, and the config calls never launch it.
    sc(vec!["delete", TEST_SVC]);
    if !sc(vec![
        "create",
        TEST_SVC,
        "binPath=",
        "C:\\Windows\\System32\\cmd.exe",
        "start=",
        "demand",
    ]) {
        panic!("could not create the throwaway service, is this an admin shell?");
    }
    println!("created {TEST_SVC}");

    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG
        | ServiceAccess::CHANGE_CONFIG;
    let svc = manager.open_service(TEST_SVC, access).expect("open it");
    let status = svc.query_status().expect("status");
    println!("state = {:?}", status.current_state);

    // The real risk here. change_config rewrites the whole service record, so a
    // field the bridge fails to carry over would be silently wiped. Each pass
    // checks the start type took and that the binary path survived unchanged.
    let before = binpath();
    let wanted = vec![
        ServiceStartType::Disabled,
        ServiceStartType::AutoStart,
        ServiceStartType::OnDemand,
    ];
    for want in wanted {
        // A whole ServiceInfo, the way the real crate takes it. Everything
        // except the start type is read straight back off the current config,
        // which is what makes this a round trip rather than a rewrite.
        let cfg = svc.query_config().expect("read the current config");
        let info = ServiceInfo {
            name: TEST_SVC.into(),
            display_name: cfg.display_name,
            service_type: cfg.service_type,
            start_type: want,
            error_control: cfg.error_control,
            executable_path: cfg.executable_path,
            launch_arguments: vec![],
            dependencies: cfg.dependencies,
            account_name: cfg.account_name,
            account_password: None,
        };
        svc.change_config(&info).expect("change the start type");
        let got = svc.query_config().expect("read it back").start_type;
        println!("set {want:?} -> {got:?}");
        assert_eq!(format!("{got:?}"), format!("{want:?}"));
        assert_eq!(binpath(), before, "the binary path was not preserved");
    }

    sc(vec!["delete", TEST_SVC]);
    println!("deleted {TEST_SVC}");

    check_start_stop(manager);
    println!("all service write paths passed");
}

// Start and stop against the print spooler, putting it back the way it was
// found. Nothing on a dev box depends on it, and unlike the throwaway above it
// is a real service that can actually reach Running.
fn check_start_stop(manager: ServiceManager) {
    let access = ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP;
    let Ok(spooler) = manager.open_service("Spooler", access) else {
        println!("no Spooler service here, skipping the start and stop checks");
        return;
    };
    let original = spooler.query_status().expect("read it").current_state;
    println!("spooler starts out {original:?}");

    spooler.stop().expect("stop the spooler");
    wait_for(&spooler, ServiceState::Stopped);
    println!("stop -> Stopped");

    spooler.start().expect("start the spooler");
    wait_for(&spooler, ServiceState::Running);
    println!("start -> Running");

    if format!("{original:?}") == "Stopped" {
        spooler.stop().expect("put the spooler back");
        println!("spooler put back to Stopped");
    }
}

// Start and stop are asynchronous, so the state is polled rather than read once.
fn wait_for(svc: &Service, want: ServiceState) {
    for _ in 0..150 {
        let now = svc.query_status().expect("status").current_state;
        if format!("{now:?}") == format!("{want:?}") {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("timed out waiting for {want:?}");
}
