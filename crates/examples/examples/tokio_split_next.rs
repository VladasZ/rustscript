// `split(..).next()` on the parallel engine that `#[tokio::main]` selects.
// Iterators are eager lists there, so `next` answers the first element. st.rs
// peels a worktree's canonical repo out of a git dir path exactly this way.

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
