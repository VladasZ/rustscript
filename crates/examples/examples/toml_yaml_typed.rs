use serde::Deserialize;

#[derive(Deserialize)]
struct Outer {
    name: String,
    inner: Inner,
    #[serde(default)]
    tags: Vec<String>,
    note: Option<String>,
}

#[derive(Deserialize)]
struct Inner {
    value: String,
    #[serde(default = "seven")]
    count: u8,
}

fn seven() -> u8 {
    7
}

fn toml_outer(text: &str) {
    let parsed: Result<Outer, toml::de::Error> = toml::from_str(text);
    match parsed {
        Ok(outer) => show(&outer),
        Err(e) => println!("err {e}"),
    }
}

fn yaml_outer(text: &str) {
    let parsed: Result<Outer, serde_yaml::Error> = serde_yaml::from_str(text);
    match parsed {
        Ok(outer) => show(&outer),
        Err(e) => println!("err {e}"),
    }
}

fn main() {
    toml_outer("other = 1");
    toml_outer("name = \"a\"\n[inner]\nother = 2\n");
    toml_outer("name = \"a\"\ntags = [\"x\"]\n[inner]\nvalue = \"v\"\n");
    yaml_outer("other: 1\n");
    yaml_outer("name: a\ninner:\n  value: v\n  count: 2\nnote: hi\n");
}

fn show(outer: &Outer) {
    println!(
        "ok {} {} {} {:?} {:?}",
        outer.name, outer.inner.value, outer.inner.count, outer.tags, outer.note
    );
}
