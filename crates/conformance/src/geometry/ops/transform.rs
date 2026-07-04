use super::super::shapes::Rect;

pub fn translate(r: &Rect, dx: i64, dy: i64) -> Rect {
    Rect { w: r.w + dx, h: r.h + dy }
}

pub fn scale(r: &Rect, k: i64) -> Rect {
    Rect { w: r.w * k, h: r.h * k }
}
