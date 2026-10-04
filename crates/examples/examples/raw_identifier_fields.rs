#!/usr/bin/env rust

//! A field written as a raw identifier, `r#type`, has the name `type`. `Debug`, `Serialize`
//! and `Deserialize` use the name without the `r#`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Info {
    r#type: String,
    r#match: u8,
    plain: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
struct Loud {
    r#fn: i32,
}

#[derive(Debug, Serialize, Deserialize)]
enum Shape {
    Named { r#ref: u8, size: u8 },
}

fn main() {
    let info = Info {
        r#type: "web".to_string(),
        plain: true,
        r#match: 7,
    };
    println!("{info:?}");
    println!("{}", serde_json::to_string(&info).unwrap());
    let parsed: Info = serde_json::from_str(r#"{"type":"tv","match":3,"plain":false}"#).unwrap();
    println!("{parsed:?} {} {}", parsed.r#type, parsed.r#match + 1);
    let Info {
        r#type,
        r#match: count,
        ..
    } = parsed.clone();
    println!("{type} {count}");
    let mut edited = Info {
        r#match: 9,
        ..parsed
    };
    edited.r#type.push('!');
    println!("{edited:?} {}", edited == Info::default());
    println!(
        "{:?}",
        serde_json::from_str::<Info>(r#"{"r#type":"x","match":1,"plain":true}"#)
            .map_err(|e| e.to_string())
    );

    let loud = Loud { r#fn: -2 };
    println!("{loud:?} {}", serde_json::to_string(&loud).unwrap());
    let shape = Shape::Named { size: 2, r#ref: 1 };
    println!("{shape:?} {}", serde_json::to_string(&shape).unwrap());
    match shape {
        Shape::Named { r#ref, size } => println!("{ref} {size}"),
    }
}
