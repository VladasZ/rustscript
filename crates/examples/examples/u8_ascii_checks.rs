#!/usr/bin/env rust

//! The ASCII helpers of `u8`, called as methods and passed as paths like `u8::is_ascii_hexdigit`.

fn is_hex_color(text: &str) -> bool {
    match text.strip_prefix('#') {
        Some(digits) => {
            matches!(digits.len(), 3 | 6) && digits.bytes().all(|b| b.is_ascii_hexdigit())
        }
        None => false,
    }
}

fn flags(byte: u8) -> String {
    let checks = [
        byte.is_ascii(),
        byte.is_ascii_alphabetic(),
        byte.is_ascii_alphanumeric(),
        byte.is_ascii_control(),
        byte.is_ascii_digit(),
        byte.is_ascii_graphic(),
        byte.is_ascii_hexdigit(),
        byte.is_ascii_lowercase(),
        byte.is_ascii_punctuation(),
        byte.is_ascii_uppercase(),
        byte.is_ascii_whitespace(),
    ];
    checks
        .iter()
        .map(|&on| if on { '1' } else { '0' })
        .collect()
}

fn main() {
    let bytes = "a9z".as_bytes();
    let all = bytes.iter().all(u8::is_ascii_hexdigit);
    println!("{} {all}", bytes[0].is_ascii_hexdigit());

    for byte in [
        b'a', b'F', b'g', b'7', b'_', b' ', b'\n', 0u8, 127, 128, 255,
    ] {
        println!("{byte:>3} {}", flags(byte));
    }

    let text = b"Fe 2+\tx";
    let digits = text.iter().filter(|b| b.is_ascii_digit()).count();
    let spaces: Vec<bool> = text.iter().map(u8::is_ascii_whitespace).collect();
    let upper: Vec<u8> = text.iter().map(u8::to_ascii_uppercase).collect();
    let lower: Vec<u8> = text.iter().map(u8::to_ascii_lowercase).collect();
    println!("{digits} {spaces:?}");
    println!("{upper:?}");
    println!("{}", String::from_utf8_lossy(&lower));

    let wide: u8 = 200;
    println!(
        "{} {} {}",
        wide.is_ascii(),
        wide.to_ascii_uppercase(),
        wide.is_ascii_graphic()
    );
    println!(
        "{} {} {}",
        b'q'.eq_ignore_ascii_case(&b'Q'),
        b'q'.eq_ignore_ascii_case(&b'r'),
        text[0].eq_ignore_ascii_case(&b'f')
    );

    for color in ["#a9f", "#A0b1C2", "#a9z", "a9f", "#a9f0"] {
        println!("{color} {}", is_hex_color(color));
    }

    println!(
        "{} {} {} {}",
        '\t'.is_ascii_control(),
        'x'.is_ascii_control(),
        'x'.is_ascii_graphic(),
        ' '.is_ascii_graphic()
    );
}
