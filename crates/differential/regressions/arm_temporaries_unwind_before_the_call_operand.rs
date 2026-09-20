//! A panic inside a match arm that is an argument of a call. The arm is a scope inside the
//! call, so its temporaries and bindings unwind first, then the receiver the call was about
//! to take, and the temporary of the statement around the call last.

#[derive(Clone, Debug)]
struct Trace(i64);

impl Drop for Trace {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn five() -> usize {
    5
}

fn label(trace: &Trace) -> i64 {
    trace.0
}

fn main() {
    let flag = five() == 5;
    let kept = label(&Trace(9))
        + Some(Trace(1))
            .unwrap_or(match flag {
                true => {
                    let inner = Trace(3);
                    vec![Trace(2), inner.clone()][five()].clone()
                }
                false => Trace(4),
            })
            .0;
    println!("{kept}");
}
