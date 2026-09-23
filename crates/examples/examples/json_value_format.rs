use serde_json::{Value, json};

fn describe(v: &Value) -> String {
    format!("<{v}>")
}

fn main() {
    let v: Value = serde_json::from_str(r#"{"a":{"c":-1},"b":[1,2.5,null,true,"s"]}"#).unwrap();
    println!("{v}");
    println!("{v:?}");
    println!("{v:#}");
    println!("{v:#?}");
    println!("{}", v["b"][4]);
    println!("{}", v["b"][1]);
    println!("{:?}", v["b"][4]);
    let name = &v["a"];
    println!("inline {name} and {}", describe(name));
    let whole = v.to_string();
    println!("{whole}");
    let built = json!({"list": ["x", 2], "z": 1});
    println!("{built}");
    let s = format!("{built:?}");
    println!("{s}");
    println!("{} {} {}", v["missing"], v["b"][9], v["a"]["c"]);
    println!("{} {} {}", v["b"][4]["x"], v["b"]["key"], v[0]);
    println!("{:?} {}", v["a"]["zz"].as_str(), v["missing"].is_null());
    let text = v["b"][4].to_string();
    println!("{text} {}", text.len());
}
