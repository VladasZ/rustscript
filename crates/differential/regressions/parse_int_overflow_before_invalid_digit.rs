// The digits overflow the target before the newline is read, so the message is the overflow,
// not the invalid digit. Seed 20688205074.

fn main() {
    println!("{:?}", vec![String::from("99999999999999999999"), String::from("\n")].concat().parse::<usize>().map_err(|e| e.to_string()));
    println!("{:?}", "99999999999999999999x".parse::<usize>().map_err(|e| e.to_string()));
    println!("{:?}", "-99999999999999999999x".parse::<i64>().map_err(|e| e.to_string()));
    println!("{:?}", "-99999999999999999999x".parse::<u64>().map_err(|e| e.to_string()));
    println!("{:?}", "300x".parse::<u8>().map_err(|e| e.to_string()));
    println!("{:?}", "1x".parse::<u8>().map_err(|e| e.to_string()));
    println!("{:?}", "-129".parse::<i8>().map_err(|e| e.to_string()));
    println!("{:?}", "-".parse::<i8>().map_err(|e| e.to_string()));
    println!("{:?}", "+".parse::<u8>().map_err(|e| e.to_string()));
    println!("{:?}", "+7".parse::<u8>());
    println!("{:?}", "-0".parse::<u8>().map_err(|e| e.to_string()));
    println!("{:?}", "340282366920938463463374607431768211456".parse::<u128>().map_err(|e| e.to_string()));
    println!("{:?}", "340282366920938463463374607431768211455".parse::<u128>());
    println!("{:?}", "-170141183460469231731687303715884105729".parse::<i128>().map_err(|e| e.to_string()));
}
