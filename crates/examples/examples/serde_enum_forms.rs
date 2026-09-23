#!/usr/bin/env rust


use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
enum Plain {
    Unit,
    New(i32),
    Pair(i32, String),
    Rec { a: u8, b: Option<String> },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Started { at: u64 },
    Stopped,
    Moved(Pos),
}

#[derive(Debug, Deserialize, Serialize)]
struct Pos {
    x: i32,
    y: i32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "t", content = "c")]
enum Msg {
    Ping,
    Text(String),
    Many(u8, u8),
    Named { n: i64 },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum Any {
    Num(i64),
    Word(String),
    List(Vec<i64>),
    Obj { id: u32 },
    Nothing,
}

#[derive(Debug, Deserialize, Serialize)]
enum Level {
    #[serde(rename = "warn")]
    Warning,
    Error,
}

#[derive(Debug, Deserialize, Serialize)]
struct Doc {
    level: Level,
    events: Vec<Event>,
    maybe: Option<Plain>,
}

fn show<T: std::fmt::Debug + Serialize>(r: Result<T, serde_json::Error>) {
    match r {
        Ok(v) => println!("ok {v:?} => {}", serde_json::to_string(&v).unwrap()),
        Err(e) => println!("err {e}"),
    }
}

fn main() {
    show(serde_json::from_str::<Vec<Plain>>(
        r#"["Unit",{"New":5},{"Pair":[1,"x"]},{"Rec":{"a":2}}]"#,
    ));
    show(serde_json::from_str::<Plain>(r#""Nope""#));
    show(serde_json::from_str::<Vec<Event>>(
        r#"[{"type":"started","at":9},{"type":"stopped"},{"type":"moved","x":1,"y":2}]"#,
    ));
    show(serde_json::from_str::<Event>(r#"{"at":1}"#));
    show(serde_json::from_str::<Vec<Msg>>(
        r#"[{"t":"Ping"},{"t":"Text","c":"hi"},{"t":"Many","c":[1,2]},{"t":"Named","c":{"n":-3}}]"#,
    ));
    show(serde_json::from_str::<Vec<Any>>(
        r#"[7,"w",[1,2],{"id":4},null]"#,
    ));
    show(serde_json::from_str::<Any>("true"));
    show(serde_json::from_str::<Doc>(
        r#"{"level":"warn","events":[{"type":"stopped"}],"maybe":{"New":1}}"#,
    ));
    show(serde_json::from_str::<Doc>(
        r#"{"level":"Error","events":[],"maybe":null}"#,
    ));
    let built = vec![
        Event::Started { at: 3 },
        Event::Moved(Pos { x: 0, y: -1 }),
        Event::Stopped,
    ];
    println!("{}", serde_json::to_string(&built).unwrap());
    let p = Plain::Rec {
        b: Some("q".into()),
        a: 1,
    };
    println!("{p:?} {}", serde_json::to_string(&p).unwrap());
    if let Ok(Event::Started { at }) =
        serde_json::from_str::<Event>(r#"{"type":"started","at":42}"#)
    {
        println!("matched {at}");
    }
    let t: Result<Plain, _> = toml::from_str::<Doc>("level = \"Error\"\nevents = []\n")
        .map(|d| d.maybe.unwrap_or(Plain::Unit));
    println!("{t:?}");
}
