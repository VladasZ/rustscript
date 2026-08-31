//! A named constant in a pattern compares against its value. It used to compile to a dead arm, so
//! `Some(REBOOT_EXIT)` never matched and a reboot request was reported as failed.

const REBOOT_EXIT: i32 = 3;

fn main() {
    let code = Some(3);
    match code {
        Some(0) => println!("ok"),
        Some(REBOOT_EXIT) => println!("reboot"),
        _ => println!("failed"),
    }

    const LOCAL: u8 = 7;
    match 7u8 {
        LOCAL => println!("local"),
        _ => println!("no local"),
    }
}
