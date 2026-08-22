//! Reading floats back out of raw bytes, which is what a binary file format
//! needs. The width matters: four bytes read as f32 is not the f64 read of
//! eight, so each width has to keep its own path.

fn main() {
    let narrow: f32 = 12.375;
    println!("f32 le {:?}", f32::from_le_bytes(narrow.to_le_bytes()));
    println!("f32 be {:?}", f32::from_be_bytes(narrow.to_be_bytes()));
    println!("f32 ne {:?}", f32::from_ne_bytes(narrow.to_ne_bytes()));
    println!("f32 byte count {}", narrow.to_le_bytes().len());

    let wide: f64 = 1234.5;
    println!("f64 le {:?}", f64::from_le_bytes(wide.to_le_bytes()));
    println!("f64 be {:?}", f64::from_be_bytes(wide.to_be_bytes()));
    println!("f64 ne {:?}", f64::from_ne_bytes(wide.to_ne_bytes()));
    println!("f64 byte count {}", wide.to_le_bytes().len());

    // a literal array, the shape a file reader builds by hand
    println!("literal f32 {:?}", f32::from_le_bytes([0, 0, 128, 63]));
    println!(
        "literal f64 {:?}",
        f64::from_le_bytes([0, 0, 0, 0, 0, 0, 240, 63])
    );
    println!("literal f32 pi {:?}", f32::from_le_bytes([219, 15, 73, 64]));

    // widening an f32 read is not an f64 read of the same eight bytes
    let widened = f64::from(f32::from_le_bytes([219, 15, 73, 64]));
    println!("widened {widened:?}");

    // negative, zero and the byte order actually differing
    println!("neg {:?}", f32::from_le_bytes((-2.5_f32).to_le_bytes()));
    println!("zero {:?}", f64::from_le_bytes(0.0_f64.to_le_bytes()));
    println!("le bytes {:?}", 1.0_f32.to_le_bytes());
    println!("be bytes {:?}", 1.0_f32.to_be_bytes());
}
