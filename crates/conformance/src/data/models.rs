pub const DEFAULT_TAG: &str = "general";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Tool,
    Food,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rating {
    Unrated,
    Stars(i64),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub kind: Kind,
}

impl Item {
    pub fn new(id: i64, name: &str, kind: Kind) -> Item {
        Item {
            id,
            name: name.to_string(),
            kind,
        }
    }

    pub fn describe(&self) -> String {
        let kind = match &self.kind {
            Kind::Tool => "tool",
            Kind::Food => "food",
        };
        format!("{} #{} [{}]", self.name, self.id, kind)
    }
}
