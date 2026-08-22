// `split(..).next()` under `#[tokio::main]`, the shape `st.rs` uses.

#[tokio::main]
async fn main() {
    let git_dir = "/repo/.git/worktrees/repo-LL-1";
    let canonical = git_dir.split("/.git/worktrees/").next().unwrap();
    println!("canonical {canonical}");

    let first = "a,b,c".split(',').next().unwrap();
    println!("first {first}");

    let whole = "abc".split('x').next().unwrap();
    println!("whole {whole}");
}
