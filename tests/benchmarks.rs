mod common;
use common::Fixture;
use git2::Sort;
use std::time::Instant;

#[test]
fn compare_sortings() {
    let f = Fixture::synthetic("bench", 10_000);
    for (label, sort) in [
        ("TOPOLOGICAL|TIME", Sort::TOPOLOGICAL | Sort::TIME),
        ("TOPOLOGICAL", Sort::TOPOLOGICAL),
        ("TIME", Sort::TIME),
        ("NONE", Sort::NONE),
    ] {
        for limit in [1_000usize, 10_000] {
            let start = Instant::now();
            let mut walk = f.repo.revwalk().unwrap();
            walk.set_sorting(sort).unwrap();
            walk.push_glob("refs/heads/*").unwrap();
            let mut n = 0;
            for oid in walk {
                let _ = oid.unwrap();
                n += 1;
                if n >= limit {
                    break;
                }
            }
            println!(
                "{label:18} limit {limit:6}: {:?} ({n} commits)",
                start.elapsed()
            );
        }
    }

    // Where does the time actually go?
    let start = Instant::now();
    let mut walk = f.repo.revwalk().unwrap();
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME).unwrap();
    walk.push_glob("refs/heads/*").unwrap();
    let setup = start.elapsed();
    let start2 = Instant::now();
    let first = walk.next();
    println!(
        "push+sort: {setup:?}, first next(): {:?} ({:?})",
        start2.elapsed(),
        first.is_some()
    );

    let start3 = Instant::now();
    let rest: Vec<_> = walk.collect();
    println!("remaining {} commits: {:?}", rest.len(), start3.elapsed());

    // And how much does turning oids into summaries cost?
    let mut walk = f.repo.revwalk().unwrap();
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME).unwrap();
    walk.push_glob("refs/heads/*").unwrap();
    let oids: Vec<_> = walk.take(10_000).map(|o| o.unwrap()).collect();
    let start4 = Instant::now();
    for oid in &oids {
        let c = f.repo.find_commit(*oid).unwrap();
        let _ = c.summary();
        let _ = c.author().name().map(str::to_owned);
    }
    println!(
        "find_commit+summarize {} commits: {:?}",
        oids.len(),
        start4.elapsed()
    );
}
