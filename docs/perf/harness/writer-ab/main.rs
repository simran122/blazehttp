//! One-binary A/B of three `write_request` revisions (pre-review, fix round 1, current) plus the current baselines,
//! on the committed request sets, in rotated order. Every variant is checked to write the current writer's bytes.
//! `-- --verify` checks only.

mod write_bench;
mod write_rings;

use engine_core::h1::{HeaderTemplate, write_request};
use write_bench::{
    Framed, frame, measure_variants, median, memcpy, template_bytes, time_block, unchecked_appends, writer_shape,
};
use write_rings::{DEFAULT_LINES, DEFAULTS, HOST};

const VARIANTS: [&str; 6] = ["base", "fr1", "new", "shape", "appends", "memcpy"];

/// Generates `parts`, `template` and `write` for one engine-core revision.
macro_rules! revision {
    ($module:ident, $krate:ident, $template:expr) => {
        mod $module {
            use super::{DEFAULTS, Framed, HOST};
            pub use $krate::h1::HeaderTemplate;
            use $krate::h1::{RequestParts, write_request};

            pub fn template() -> HeaderTemplate {
                $template
            }

            pub fn write(out: &mut Vec<u8>, framed: &Framed, template: &HeaderTemplate) {
                let request = &framed.request;
                let method = $krate::generated::spec::Method::from_wire(request.method.to_wire()).expect("method");
                let parts = RequestParts {
                    method,
                    target: &request.target,
                    headers: &request.headers,
                    header_lines: request.header_lines,
                    body: &request.body,
                };
                let _ = write_request(out, &parts, template, HOST).expect("valid");
            }
        }
    };
}

revision!(base, engine_core_base, HeaderTemplate::new(HOST, DEFAULTS, &[]).expect("template"));
revision!(fr1, engine_core_fr1, HeaderTemplate::new(HOST, DEFAULTS, super::DEFAULT_LINES, &[]).expect("template"));

fn measure(name: &str, ring: &[Framed], template: &HeaderTemplate) -> [f64; VARIANTS.len()] {
    let template_bytes = template_bytes();
    let (base_template, fr1_template) = (base::template(), fr1::template());
    let mut out = Vec::new();
    let runs = measure_variants(name, VARIANTS, |variant| match variant {
        0 => time_block(ring, &mut out, |out, r| base::write(out, r, &base_template)),
        1 => time_block(ring, &mut out, |out, r| fr1::write(out, r, &fr1_template)),
        2 => time_block(ring, &mut out, |out, r: &Framed| {
            let _ = write_request(out, &r.request.parts(), template, HOST).expect("valid");
        }),
        3 => time_block(ring, &mut out, |out, r| writer_shape(out, r, &template_bytes)),
        4 => time_block(ring, &mut out, |out, r| unchecked_appends(out, r, &template_bytes)),
        _ => time_block(ring, &mut out, memcpy),
    });
    runs.map(|mut series| median(&mut series))
}

fn main() {
    let template = HeaderTemplate::new(HOST, DEFAULTS, DEFAULT_LINES, &[]).expect("valid template");
    let (base_template, fr1_template) = (base::template(), fr1::template());
    let sets = [("get", write_rings::get()), ("post1k", write_rings::post1k()), ("varied", write_rings::varied())];
    let rings: Vec<_> = sets.into_iter().map(|(name, ring)| (name, frame(ring, &template).0)).collect();
    let mut out = Vec::new();
    for (_, ring) in &rings {
        for r in ring {
            base::write(&mut out, r, &base_template);
            assert_eq!(out, r.wire, "base writes the current bytes");
            fr1::write(&mut out, r, &fr1_template);
            assert_eq!(out, r.wire, "fr1 writes the current bytes");
        }
    }
    if write_bench::verify_only() {
        println!("verified: every revision and baseline writes the current writer's bytes");
        return;
    }
    println!();
    println!("| set | base ns | fr1 ns | new ns | shape ns | appends ns | memcpy ns |");
    println!("|---|---|---|---|---|---|---|");
    for (name, ring) in &rings {
        let [b, f, n, s, a, m] = measure(name, ring, &template);
        println!("| `{name}` | {b:.1} | {f:.1} | {n:.1} | {s:.1} | {a:.1} | {m:.1} |");
    }
}
