use super::*;

fn same(name: BuiltinId, width: IntWidth, recv: i128, args: &[i128]) -> i128 {
    match int_method(name, width, recv, args).expect("known method") {
        Ok(IntOut::Same(value)) => value,
        Ok(_) => panic!("{name} did not answer a value"),
        Err(error) => panic!("{name} failed: {error}"),
    }
}

/// A `u64` past `i64::MAX` must not be clamped before the method sees it.
#[test]
fn a_u64_past_i64_max_keeps_its_value() {
    let big = i128::from(u64::MAX);
    assert_eq!(same(BuiltinId::Max, IntWidth::U64, big, &[0]), big);
    assert_eq!(same(BuiltinId::Min, IntWidth::U64, big, &[big]), big);
    assert_eq!(
        same(BuiltinId::SaturatingAdd, IntWidth::U64, big, &[0]),
        big
    );
}

/// saturation at the receiver's real bounds
#[test]
fn saturating_uses_the_real_width() {
    assert_eq!(
        same(BuiltinId::SaturatingAdd, IntWidth::U8, 200, &[100]),
        255
    );
    assert_eq!(
        same(BuiltinId::SaturatingSub, IntWidth::I8, -100, &[100]),
        -128
    );
    assert_eq!(same(BuiltinId::SaturatingMul, IntWidth::U8, 5, &[100]), 255);
    assert_eq!(same(BuiltinId::SaturatingSub, IntWidth::U8, 5, &[100]), 0);
}

#[test]
fn pow_and_abs_panic_where_debug_rust_panics() {
    let overflow = int_method(BuiltinId::Pow, IntWidth::U8, 16, &[2]).expect("known");
    assert!(overflow.is_err(), "16u8.pow(2) must overflow");
    assert_eq!(same(BuiltinId::Pow, IntWidth::U8, 15, &[2]), 225);

    let negate = int_method(BuiltinId::Abs, IntWidth::I8, -128, &[]).expect("known");
    assert!(negate.is_err(), "i8::MIN.abs() must overflow");
    assert_eq!(same(BuiltinId::Abs, IntWidth::I8, -127, &[]), 127);
}

/// A zero divisor must not crash the interpreter process.
#[test]
fn is_multiple_of_zero_does_not_crash() {
    let answer = int_method(BuiltinId::IsMultipleOf, IntWidth::U64, 0, &[0]).expect("known");
    assert!(matches!(answer, Ok(IntOut::Bool(true))));
    let answer = int_method(BuiltinId::IsMultipleOf, IntWidth::U64, 5, &[0]).expect("known");
    assert!(matches!(answer, Ok(IntOut::Bool(false))));
}

#[test]
fn wrapping_and_checked_follow_the_width() {
    assert_eq!(same(BuiltinId::WrappingAdd, IntWidth::U8, 250, &[10]), 4);
    assert_eq!(same(BuiltinId::WrappingSub, IntWidth::U8, 0, &[1]), 255);
    assert_eq!(same(BuiltinId::WrappingMul, IntWidth::I8, 100, &[3]), 44);
    let checked = int_method(BuiltinId::CheckedAdd, IntWidth::U8, 250, &[10]).expect("known");
    assert!(matches!(checked, Ok(IntOut::Checked(None))));
    let checked = int_method(BuiltinId::CheckedAdd, IntWidth::U8, 1, &[2]).expect("known");
    assert!(matches!(checked, Ok(IntOut::Checked(Some(3)))));
}

#[test]
fn checked_shifts_gate_on_the_width() {
    let shifted = int_method(BuiltinId::CheckedShl, IntWidth::U8, 200, &[1]).expect("known");
    assert!(matches!(shifted, Ok(IntOut::Checked(Some(144)))));
    let shifted = int_method(BuiltinId::CheckedShl, IntWidth::U8, 1, &[8]).expect("known");
    assert!(matches!(shifted, Ok(IntOut::Checked(None))));
    let shifted = int_method(BuiltinId::CheckedShr, IntWidth::I8, -128, &[2]).expect("known");
    assert!(matches!(shifted, Ok(IntOut::Checked(Some(-32)))));
    let shifted = int_method(BuiltinId::CheckedShr, IntWidth::I8, -1, &[8]).expect("known");
    assert!(matches!(shifted, Ok(IntOut::Checked(None))));
}

#[test]
fn bit_methods_use_the_width_not_the_storage() {
    let count = int_method(BuiltinId::CountOnes, IntWidth::U8, 250, &[]).expect("known");
    assert!(matches!(count, Ok(IntOut::Count(6))));
    let count = int_method(BuiltinId::LeadingZeros, IntWidth::U8, 1, &[]).expect("known");
    assert!(matches!(count, Ok(IntOut::Count(7))));
    let count = int_method(BuiltinId::TrailingZeros, IntWidth::U8, 0, &[]).expect("known");
    assert!(matches!(count, Ok(IntOut::Count(8))));
    assert_eq!(
        same(BuiltinId::SwapBytes, IntWidth::U16, 0x1234, &[]),
        0x3412
    );
    assert_eq!(
        same(BuiltinId::ReverseBits, IntWidth::U8, 0b1000_0000, &[]),
        1
    );
    assert_eq!(
        same(BuiltinId::RotateLeft, IntWidth::U8, 0b1000_0001, &[1]),
        0b11
    );
}

fn bytes(name: BuiltinId, width: IntWidth, recv: i128) -> Vec<u8> {
    match int_method(name, width, recv, &[]).expect("known method") {
        Ok(IntOut::Bytes(out)) => out,
        Ok(_) => panic!("{name} did not answer bytes"),
        Err(error) => panic!("{name} failed: {error}"),
    }
}

/// The 2 orders must disagree, or an endianness bug reads as correct.
#[test]
fn byte_conversions_keep_their_order() {
    assert_eq!(
        bytes(BuiltinId::ToLeBytes, IntWidth::U32, 0x1234_5678),
        [0x78, 0x56, 0x34, 0x12]
    );
    assert_eq!(
        bytes(BuiltinId::ToBeBytes, IntWidth::U32, 0x1234_5678),
        [0x12, 0x34, 0x56, 0x78]
    );
    assert_eq!(bytes(BuiltinId::ToLeBytes, IntWidth::U8, 0xab), [0xab]);
    assert_eq!(
        bytes(BuiltinId::ToBeBytes, IntWidth::U64, 1),
        [0, 0, 0, 0, 0, 0, 0, 1]
    );
    let le = from_bytes(IntWidth::U32, ByteOrder::Le, &[0x78, 0x56, 0x34, 0x12]).unwrap();
    let be = from_bytes(IntWidth::U32, ByteOrder::Be, &[0x78, 0x56, 0x34, 0x12]).unwrap();
    assert_eq!(le, 0x1234_5678);
    assert_eq!(be, 0x7856_3412);
}

/// an unsigned width reads the same bytes as a positive number
#[test]
fn byte_conversions_respect_the_sign() {
    assert_eq!(bytes(BuiltinId::ToBeBytes, IntWidth::I16, -2), [0xff, 0xfe]);
    assert_eq!(bytes(BuiltinId::ToLeBytes, IntWidth::I16, -2), [0xfe, 0xff]);
    let signed = from_bytes(IntWidth::I32, ByteOrder::Le, &[0xff, 0xff, 0xff, 0xff]).unwrap();
    let unsigned = from_bytes(IntWidth::U32, ByteOrder::Le, &[0xff, 0xff, 0xff, 0xff]).unwrap();
    assert_eq!(signed, -1);
    assert_eq!(unsigned, 0xffff_ffff);
    let low = from_bytes(IntWidth::I8, ByteOrder::Be, &[0x80]).unwrap();
    assert_eq!(low, -128);
}

#[test]
fn from_bytes_rejects_a_shape_the_type_checker_would_have() {
    assert!(from_bytes(IntWidth::U32, ByteOrder::Le, &[1, 2, 3]).is_err());
    assert!(from_bytes(IntWidth::U16, ByteOrder::Le, &[1, 256]).is_err());
    assert!(from_bytes(IntWidth::U16, ByteOrder::Le, &[1, -1]).is_err());
}

#[test]
fn unknown_names_fall_through() {
    assert!(int_method(BuiltinId::Sqrt, IntWidth::I64, 4, &[]).is_none());
    assert!(int_method(BuiltinId::Abs, IntWidth::U8, 4, &[]).is_none());
    assert!(int_method(BuiltinId::Signum, IntWidth::U8, 4, &[]).is_none());
}
