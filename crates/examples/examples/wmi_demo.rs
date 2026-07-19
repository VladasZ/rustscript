#!/usr/bin/env rust

// Queries WMI through the wmi bridge. Read only.
//
// Windows only. The examples suite skips it elsewhere.

use wmi::WMIConnection;

fn main() {
    let wmi = WMIConnection::new().expect("connect to root\\cimv2");

    // The same class drivers.ps1 reads to gate itself to a known model.
    let rows = wmi
        .raw_query("SELECT Name, Version FROM Win32_ComputerSystemProduct")
        .expect("query the computer system product");

    assert!(!rows.is_empty());
    for row in rows {
        println!("Name = {:?}", row.get("Name"));
        println!("Version = {:?}", row.get("Version"));
    }

    // A bad query reports an error rather than panicking.
    let bad = wmi.raw_query("SELECT * FROM Win32_NoSuchClassHere");
    assert!(bad.is_err());
    println!("bad query reports an error, as expected");
}
