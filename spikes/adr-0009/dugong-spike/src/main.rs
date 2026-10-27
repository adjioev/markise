use dugong::graphlib::{Graph, GraphOptions};
use dugong::{EdgeLabel, GraphLabel, NodeLabel, WorkControl, WorkError};
use std::time::{Duration, Instant};

struct Deadline(Instant, usize);
impl WorkControl for Deadline {
    fn charge(&mut self, units: usize) -> Result<(), WorkError> {
        self.1 += units;
        if Instant::now() > self.0 { Err(WorkError::Interrupted) } else { Ok(()) }
    }
}

fn graph(n: usize, edges: &[(usize, usize)], compound: bool) -> Graph<NodeLabel, EdgeLabel, GraphLabel> {
    let mut g: Graph<NodeLabel, EdgeLabel, GraphLabel> = Graph::new(GraphOptions { directed: true, multigraph: true, compound });
    g.set_graph(GraphLabel { nodesep: 50.0, ranksep: 50.0, ..Default::default() });
    g.set_default_node_label(NodeLabel::default);
    g.set_default_edge_label(|| EdgeLabel { minlen: 1, weight: 1.0, ..Default::default() });
    for i in 0..n { g.set_node(format!("n{i}"), NodeLabel { width: 80.0, height: 36.0, ..Default::default() }); }
    for &(a, b) in edges { g.set_edge_with_label(format!("n{a}"), format!("n{b}"), EdgeLabel { minlen: 1, weight: 1.0, width: 30.0, height: 14.0, ..Default::default() }); }
    g
}

fn main() {
    let mut seed: u64 = 0x9e37_79b9;
    let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    for &(n, m) in &[(6usize, 6usize), (50, 60), (200, 200), (500, 600), (1000, 1200), (2000, 2000)] {
        let edges: Vec<(usize, usize)> = (0..m).map(|i| (i % n, (i * 7 + 3) % n)).collect();
        let mut g = graph(n, &edges, false);
        let t = Instant::now();
        let mut ctl = Deadline(Instant::now() + Duration::from_secs(3), 0);
        let r = dugong::layout_controlled(&mut g, &mut ctl);
        println!("dugong n={n} m={m}: {:?} {:?} work={}", t.elapsed(), r.as_ref().err(), ctl.1);
        // rust-sugiyama
        let e32: Vec<(u32, u32)> = edges.iter().map(|&(a, b)| (a as u32, b as u32)).collect();
        let t = Instant::now();
        let r = std::panic::catch_unwind(|| rust_sugiyama::from_edges(&e32, &rust_sugiyama::configure::Config::default()));
        println!("  sugiyama: {:?} ok={}", t.elapsed(), r.is_ok());
    }
    // random graph fuzz, incl. self loops and multi-edges
    let mut panics = 0; let t0 = Instant::now();
    for _ in 0..2000 {
        let n = (rnd() % 30 + 1) as usize; let m = (rnd() % 60) as usize;
        let edges: Vec<(usize, usize)> = (0..m).map(|_| ((rnd() as usize) % n, (rnd() as usize) % n)).collect();
        let r = std::panic::catch_unwind(|| { let mut g = graph(n, &edges, true); dugong::layout(&mut g).map(|_| ()) });
        if r.is_err() { panics += 1; }
    }
    println!("random fuzz: panics {panics} in {:?}", t0.elapsed());
}
