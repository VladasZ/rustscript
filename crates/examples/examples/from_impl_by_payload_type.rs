//! `into()` picks the `From` impl by the source type, `From<Option<usize>>` and
//! `From<Option<u16>>` are different impls. Seed 20686200357.

#[derive(Debug)]
enum Wrapped {
    Wide(Option<usize>),
    Narrow(Option<u16>),
    Pair((u8, bool)),
    Flags(Vec<bool>),
    Letters(Vec<char>),
}

impl Wrapped {
    fn tag(&self) -> String {
        match self {
            Self::Wide(inner) => format!("wide {inner:?}"),
            Self::Narrow(inner) => format!("narrow {inner:?}"),
            Self::Pair(inner) => format!("pair {inner:?}"),
            Self::Flags(inner) => format!("flags {inner:?}"),
            Self::Letters(inner) => format!("letters {inner:?}"),
        }
    }
}

impl From<Option<usize>> for Wrapped {
    fn from(value: Option<usize>) -> Self {
        Self::Wide(value)
    }
}

impl From<Option<u16>> for Wrapped {
    fn from(value: Option<u16>) -> Self {
        Self::Narrow(value)
    }
}

impl From<(u8, bool)> for Wrapped {
    fn from(value: (u8, bool)) -> Self {
        Self::Pair(value)
    }
}

impl From<Vec<bool>> for Wrapped {
    fn from(value: Vec<bool>) -> Self {
        Self::Flags(value)
    }
}

impl From<Vec<char>> for Wrapped {
    fn from(value: Vec<char>) -> Self {
        Self::Letters(value)
    }
}

fn opaque_usize(v: usize) -> usize {
    v
}

fn opaque_u16(v: u16) -> u16 {
    v
}

fn opaque_u8(v: u8) -> u8 {
    v
}

fn main() {
    let wide: Wrapped = Some(opaque_usize(2)).into();
    println!("{wide:?} {}", wide.tag());

    let narrow: Wrapped = Some(opaque_u16(3)).into();
    println!("{narrow:?} {}", narrow.tag());

    let pair: Wrapped = (opaque_u8(7), true).into();
    println!("{pair:?} {}", pair.tag());

    // apart by element type
    let flags: Wrapped = vec![false, true].into();
    println!("{flags:?} {}", flags.tag());

    let letters: Wrapped = vec!['x', 'y'].into();
    println!("{letters:?} {}", letters.tag());

    // `None` names no inner type and still reaches an impl
    let empty: Wrapped = None::<u16>.into();
    println!("{empty:?} {}", empty.tag());
}
