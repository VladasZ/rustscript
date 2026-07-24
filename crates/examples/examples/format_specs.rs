// The format spec surface: width, fill, alignment, sign, sign-aware zero
// padding, precision, radix and exponent types, and their `#` alternate
// forms, over integers, floats, and strings.

fn main() {
    let number = 255i64;
    let negative = -255i64;
    println!("[{number:>8}] [{number:<8}] [{number:^8}] [{number:08}]");
    println!("[{number:+}] [{negative:+}] [{number:+09}]");
    println!("[{number:x}] [{number:X}] [{number:#x}] [{number:#X}]");
    println!("[{number:o}] [{number:#o}] [{number:b}] [{number:#b}]");
    println!("[{number:#010x}] [{negative:x}] [{negative:#x}]");
    println!("[{:e}] [{:E}] [{:e}]", 1000i64, 1234.5f64, 0.00125f64);
    println!("[{number:*>12}] [{number:*^12}] [{number:->6}]");

    let float = 1234.56789f64;
    println!("[{float:.3}] [{float:.0}] [{float:+.2}] [{float:10.3}]");
    println!("[{float:08.2}] [{float:>14.4}] [{:.7}]", 0.1f64);
    println!("[{:+.2}] [{:08}] [{:+}]", f64::NAN, f64::NAN, f64::NAN);
    println!("[{:+.2}] [{:08.1}]", f64::INFINITY, f64::NEG_INFINITY);

    let text = "rust λ";
    println!("[{text:>10}] [{text:<10}] [{text:^10}]");
    println!("[{text:.3}] [{text:10.4}] [{text:-^12}] [{text:.0}]");

    let width = 9usize;
    let precision = 2usize;
    println!("[{number:width$}] [{float:.precision$}] [{float:width$.precision$}]");
    println!("[{0:>2$}] [{1:.3$}]", number, float, 7usize, 1usize);
}
