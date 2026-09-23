#!/usr/bin/env rust


use std::fmt;
use std::str::FromStr;

#[derive(Debug, Default, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

impl FromStr for Point {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (left, right) = text.split_once(',').ok_or("no comma")?;
        let x = left.trim().parse().map_err(|e| format!("x: {e}"))?;
        let y = right.trim().parse().map_err(|e| format!("y: {e}"))?;
        Ok(Point { x, y })
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

#[derive(Debug, Clone, Copy)]
enum Color {
    Red,
    Green,
}

impl FromStr for Color {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        match s.to_ascii_lowercase().as_str() {
            "red" => Ok(Color::Red),
            "green" => Ok(Color::Green),
            _ => Err(()),
        }
    }
}

fn shift(p: Point) -> Point {
    Point {
        x: p.x + 1,
        y: p.y - 1,
    }
}

fn read_all(lines: &[&str]) -> Result<Vec<Point>, String> {
    let mut out = Vec::new();
    for line in lines {
        out.push(line.parse()?);
    }
    Ok(out)
}

fn main() -> Result<(), String> {
    let p: Point = "3, -4".parse().unwrap();
    println!("{p} {:?}", "x,1".parse::<Point>());
    println!("{:?}", "7".parse::<Point>());
    println!("{}", shift("10,10".parse()?));
    println!("{:?}", read_all(&["1,2", "3,4"]));
    println!("{:?}", read_all(&["1,2", "3;4"]));
    let colors: Vec<Color> = "Red green blue"
        .split(' ')
        .filter_map(|s| s.parse().ok())
        .collect();
    println!("{colors:?} {:?}", "GREEN".parse::<Color>());
    let n: u8 = "42".parse().unwrap();
    println!("{n} {:?}", "300".parse::<u8>().is_err());
    Ok(())
}
