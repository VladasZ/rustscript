use serde::Deserialize;

#[derive(Deserialize)]
struct Row {
    small: u8,
    signed: i8,
    big: u64,
    ratio: f32,
    flag: bool,
    letter: char,
    name: String,
    maybe: Option<u16>,
    list: Vec<u32>,
}

fn show(row: &Row) -> String {
    format!(
        "{} {} {} {} {} {} {} {:?} {:?}",
        row.small,
        row.signed,
        row.big,
        row.ratio,
        row.flag,
        row.letter,
        row.name,
        row.maybe,
        row.list
    )
}

fn json(text: &str) {
    match serde_json::from_str::<Row>(text) {
        Ok(row) => {
            println!("json ok {}", show(&row));
            println!("wrap {:?}", row.small.checked_add(250));
        }
        Err(e) => println!("json err {e}"),
    }
}

fn toml_row(text: &str) {
    match toml::from_str::<Row>(text) {
        Ok(row) => println!("toml ok {}", show(&row)),
        Err(e) => println!("toml err {e}"),
    }
}

fn yaml(text: &str) {
    match serde_yaml::from_str::<Row>(text) {
        Ok(row) => println!("yaml ok {}", show(&row)),
        Err(e) => println!("yaml err {e}"),
    }
}

const GOOD: &str = r#"{"small":7,"signed":-3,"big":18446744073709551615,"ratio":0.1,"flag":true,"letter":"z","name":"n","maybe":null,"list":[1,2]}"#;

fn main() {
    json(GOOD);
    json(&GOOD.replace("\"small\":7", "\"small\":300"));
    json(&GOOD.replace("\"signed\":-3", "\"signed\":-200"));
    json(&GOOD.replace("\"small\":7", "\"small\":-1"));
    json(&GOOD.replace("\"small\":7", "\"small\":\"7\""));
    json(&GOOD.replace("\"flag\":true", "\"flag\":1"));
    json(&GOOD.replace("\"letter\":\"z\"", "\"letter\":\"zz\""));
    json(&GOOD.replace("\"maybe\":null", "\"maybe\":70000"));
    json(&GOOD.replace("\"maybe\":null", "\"maybe\":9"));
    json(&GOOD.replace("[1,2]", "[1,-2]"));
    json(&GOOD.replace("\"small\":7", "\"small\":7.5"));
    let t = "small = 7\nsigned = -3\nbig = 5\nratio = 0.5\nflag = false\nletter = \"q\"\nname = \"t\"\nlist = [3]\n";
    toml_row(t);
    toml_row(&t.replace("small = 7", "small = 256"));
    toml_row(&t.replace("list = [3]", "list = [\"x\"]"));
    let y = "small: 7\nsigned: -3\nbig: 5\nratio: 0.5\nflag: true\nletter: q\nname: y\nmaybe: 4\nlist: [3]\n";
    yaml(y);
    yaml(&y.replace("small: 7", "small: 999"));
}
