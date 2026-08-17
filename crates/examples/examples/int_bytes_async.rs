// Integer byte conversions under `#[tokio::main]`, in both directions and
// both byte orders.

fn hex(bytes: &[u8]) -> String {
    let parts: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    parts.join(" ")
}

#[tokio::main]
async fn main() {
    let word: u32 = 0x1234_5678;
    println!("u32 le: {}", hex(&word.to_le_bytes()));
    println!("u32 be: {}", hex(&word.to_be_bytes()));
    println!("i16 -2 be: {}", hex(&(-2i16).to_be_bytes()));
    println!("i16 -2 le: {}", hex(&(-2i16).to_le_bytes()));
    println!("u32 ne is le: {}", word.to_ne_bytes() == word.to_le_bytes());

    let raw = [0x78u8, 0x56, 0x34, 0x12];
    println!("u32 from le: {}", u32::from_le_bytes(raw));
    println!("u32 from be: {}", u32::from_be_bytes(raw));

    let high = [0xffu8, 0xff, 0xff, 0xff];
    println!("u32 all ones: {}", u32::from_le_bytes(high));
    println!("i32 all ones: {}", i32::from_le_bytes(high));
    println!("i8 high: {}", i8::from_be_bytes([0x80]));
    println!("u16 high: {}", u16::from_be_bytes([0x80, 0x00]));
    println!("i16 high: {}", i16::from_be_bytes([0x80, 0x00]));
    println!(
        "u64 from be: {}",
        u64::from_be_bytes([0, 0, 0, 0, 0, 0, 1, 0])
    );

    let value: i32 = -123_456;
    println!("round: {}", i32::from_be_bytes(value.to_be_bytes()));
}
