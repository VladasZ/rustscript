#!/usr/bin/env rust

//! `Enum::Variant` states the enum, so `unwrap_or_default()` can build its default.

#[derive(Debug, Clone, PartialEq, Default)]
enum Mode {
    #[default]
    Idle,
    Busy(u8),
}

#[derive(Debug, Clone, Default)]
struct Holder {
    slot: Option<u16>,
    count: i64,
}

impl Holder {
    fn describe(&self) -> String {
        format!("{:?}/{}", self.slot, self.count)
    }
}

fn opaque_u8(v: u8) -> u8 {
    v
}

fn main() {
    let kept = Some(Mode::Busy(opaque_u8(1)))
        .or(None::<Mode>)
        .unwrap_or_default();
    println!("kept: {kept:?}");

    let through_and = Some(Mode::Busy(opaque_u8(2)))
        .and(None::<Mode>)
        .unwrap_or_default();
    println!("through_and: {through_and:?}");

    let from_unit = Some(Mode::Idle).and(None::<Mode>).unwrap_or_default();
    println!("from_unit: {from_unit:?}");

    let holder: Holder = Vec::<Holder>::new().into_iter().nth(4).unwrap_or_default();
    println!("struct: {holder:?} {}", holder.describe());
}
