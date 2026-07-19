#!/usr/bin/env rust

// Reads and writes the Windows registry through the winreg bridge. Everything
// happens under a throwaway key in HKCU that the script deletes on the way out,
// so running this never touches real settings.
//
// Windows only. The examples suite skips it elsewhere, and on another platform
// every call returns a plain error saying the registry does not exist there.

use winreg::RegKey;
use winreg::RegValue;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::enums::RegType;

const PATH: &str = r"Software\RustScriptRegistryDemo";

fn main() {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    let (key, disposition) = hkcu.create_subkey(PATH).expect("create the demo key");
    println!("created {PATH} ({disposition:?})");

    // An int writes a DWORD and a string writes REG_SZ.
    key.set_value("Count", &7u32).expect("set Count");
    key.set_value("Label", &"hello").expect("set Label");

    // Binary has no typed form, so it goes through the untyped pair. This is
    // the shape the CapsLock scancode map needs.
    let blob = RegValue {
        bytes: vec![0, 1, 2, 253, 254, 255].into(),
        vtype: RegType::REG_BINARY,
    };
    key.set_raw_value("Blob", &blob).expect("set Blob");

    let count: u32 = key.get_value("Count").expect("read Count");
    let label: String = key.get_value("Label").expect("read Label");
    let read_back = key.get_raw_value("Blob").expect("read Blob");

    println!("Count = {count}");
    println!("Label = {label}");
    println!("Blob  = {:?} as {:?}", read_back.bytes, read_back.vtype);

    assert_eq!(count, 7);
    assert_eq!(label, "hello");
    assert_eq!(read_back.bytes, blob.bytes);

    let mut names = Vec::new();
    for item in key.enum_values() {
        let (name, _value) = item.expect("enumerate a value");
        names.push(name);
    }
    names.sort();
    println!("values = {names:?}");
    assert_eq!(names.len(), 3);

    key.delete_value("Blob").expect("delete Blob");
    let mut left = 0;
    for item in key.enum_values() {
        item.expect("enumerate after delete");
        left += 1;
    }
    assert_eq!(left, 2);

    // A missing value reads back as an error, not a panic or a silent default.
    assert!(key.get_raw_value("Nope").is_err());

    hkcu.delete_subkey_all(PATH).expect("remove the demo key");
    assert!(hkcu.open_subkey(PATH).is_err());
    println!("removed {PATH}");
}
