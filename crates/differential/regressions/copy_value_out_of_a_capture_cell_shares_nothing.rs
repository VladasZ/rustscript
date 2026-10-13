// A binding a closure writes lives in a capture cell. An owned read of a tuple or an `Option`
// out of the cell is a copy with storage of its own, so a callee that drops the copy leaves
// the cell full. Same family as seed 20738009717.

struct DiffTrace(i64);

impl Drop for DiffTrace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let trace = DiffTrace(1);
    println!("{}", trace.0);
    let mut cell_pair = (1u8, Some(2u8));
    let mut write = || {
        cell_pair.0 += 1;
        cell_pair.0
    };
    println!("write {}", write());
    let twice = Some(cell_pair).unwrap_or(cell_pair);
    println!("cell {:?} {:?} {}", twice, cell_pair, vec![cell_pair; 0usize].len());
    println!("cell {:?}", cell_pair);
    let mut cell_opt = Some(7u8);
    let mut clear = || {
        cell_opt = Some(8);
    };
    clear();
    let nested = Some(cell_opt).unwrap_or(cell_opt);
    println!("opt {:?} {:?}", nested, cell_opt);
}
