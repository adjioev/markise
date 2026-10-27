use merman::svg::{RenderResourcePolicy, SvgPipeline};
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};
use std::time::{Duration, Instant};

fn render(r: &Renderer, src: &str) -> Result<String, String> {
    let control = OperationControl::new().with_deadline(Duration::from_secs(2));
    let request = SvgRequest {
        environment: SvgEnvironment::deterministic().with_resource_policy(RenderResourcePolicy::interactive()),
        pipeline: if std::env::var("NOPIPE").is_ok() { None } else { Some(SvgPipeline::resvg_safe()) },
        ..SvgRequest::default()
    };
    match r.render(RenderRequest::svg(src, control, request)) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(svg.into_parts().0),
        Ok(_) => Err("no diagram".into()),
        Err(e) => Err(e.to_string()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let site = merman::MermaidConfig::from_value(serde_json::json!({"htmlLabels": false, "flowchart": {"htmlLabels": false}, "class": {"htmlLabels": false}, "state": {"htmlLabels": false}}));
    let r = Renderer::new().with_engine(merman::Engine::new().with_site_config(site));
    if args.get(1).map(String::as_str) == Some("fuzz") {
        // mutate samples randomly, catch panics
        let mut seed: u64 = 0x1234_5678;
        let mut rnd = move || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        let samples: Vec<String> = args[2..].iter().map(|p| std::fs::read_to_string(p).unwrap()).collect();
        let alphabet = b"[](){}<>-=.|:;\"'%#&*+ox\n AB1";
        let (mut panics, mut errs, mut ok, mut slow) = (0, 0, 0, 0);
        let t0 = Instant::now();
        for i in 0..3000 {
            let mut s = samples[i % samples.len()].clone().into_bytes();
            for _ in 0..(rnd() % 8 + 1) {
                let pos = (rnd() as usize) % (s.len() + 1);
                match rnd() % 3 {
                    0 => s.insert(pos, alphabet[(rnd() as usize) % alphabet.len()]),
                    1 if pos < s.len() => { s.remove(pos); }
                    _ => { let n = (rnd() % 40) as usize; let ch = alphabet[(rnd() as usize) % alphabet.len()]; for _ in 0..n { s.insert(pos.min(s.len()), ch); } }
                }
            }
            let src = String::from_utf8_lossy(&s).into_owned();
            let t = Instant::now();
            match std::panic::catch_unwind(|| render(&r, &src)) {
                Err(_) => { panics += 1; eprintln!("PANIC on {:?}", src); }
                Ok(Ok(_)) => ok += 1,
                Ok(Err(_)) => errs += 1,
            }
            if t.elapsed() > Duration::from_millis(200) { slow += 1; }
        }
        println!("fuzz: ok {ok} err {errs} panics {panics} slow(>200ms) {slow} total {:?}", t0.elapsed());
        return;
    }
    for p in &args[1..] {
        let src = std::fs::read_to_string(p).unwrap();
        let _ = render(&r, &src); // warm
        let t = Instant::now();
        let n = 5;
        let mut out = Err(String::new());
        for _ in 0..n { out = render(&r, &src); }
        let dt = t.elapsed() / n;
        match out {
            Ok(svg) => { println!("{p}: {:?}, {} bytes, foreignObject={} style={} text={}", dt, svg.len(), svg.contains("foreignObject"), svg.matches("<style").count(), svg.matches("<text").count()); std::fs::write(format!("{p}.svg"), svg).unwrap(); }
            Err(e) => println!("{p}: {:?} ERR {e}", dt),
        }
    }
}
