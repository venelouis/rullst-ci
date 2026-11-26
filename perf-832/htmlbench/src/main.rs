//! Rendering of the /fortunes page without HTTP or a database (Rullst issue #832, phase 1): the same twelve rows
//! plus the request-time row, sorted by message, rendered by `html!` (as frameworks/rullst does), by Askama (as
//! frameworks/axum does) and, as a reference for what a plain `String` costs, by hand. Counts allocator calls and
//! times each render. Not a fix and not part of any release.

use askama::Template;
use rullst::{Markup, html};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static REALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

struct Fortune {
    id: i32,
    message: String,
}

#[derive(Template)]
#[template(path = "fortunes.html")]
struct Fortunes<'a> {
    fortunes: &'a [Fortune],
}

const MESSAGES: [&str; 12] = [
    "fortune: No such file or directory",
    "A computer scientist is someone who fixes things that aren't broken.",
    "After enough decimal places, nobody gives a damn.",
    "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1",
    "A computer program does what you tell it to do, not what you want it to do.",
    "Emacs is a nice operating system, but I prefer UNIX. \u{2014} Tom Christaensen",
    "Any program that runs right is obsolete.",
    "A list is only as strong as its weakest link. \u{2014} Donald Knuth",
    "Feature: A bug with seniority.",
    "Computers make very fast, very accurate mistakes.",
    "<script>alert(\"This should not be displayed in a browser alert box.\");</script>",
    "\u{30d5}\u{30ec}\u{30fc}\u{30e0}\u{30ef}\u{30fc}\u{30af}\u{306e}\u{30d9}\u{30f3}\u{30c1}\u{30de}\u{30fc}\u{30af}",
];

fn rows() -> Vec<Fortune> {
    let mut rows: Vec<Fortune> = MESSAGES
        .iter()
        .enumerate()
        .map(|(index, message)| Fortune { id: index as i32 + 1, message: (*message).to_string() })
        .collect();
    rows.push(Fortune { id: 0, message: "Additional fortune added at request time.".to_string() });
    rows.sort_by(|a, b| a.message.cmp(&b.message));
    rows
}

fn render_html(fortunes: &[Fortune]) -> Markup {
    html! {
        <!DOCTYPE html>
        <html><head><title>Fortunes</title></head><body><table>
        <tr><th>id</th><th>message</th></tr>
        @for fortune in fortunes {
            <tr><td>{fortune.id}</td><td>{fortune.message}</td></tr>
        }
        </table></body></html>
    }
}

fn render_askama(fortunes: &[Fortune]) -> String {
    Fortunes { fortunes }.render().unwrap_or_default()
}

/// Reference only: one pre-sized `String`, the same escaping, no intermediate strings.
fn render_plain(fortunes: &[Fortune]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(1500);
    out.push_str("<!DOCTYPE html><html><head><title>Fortunes</title></head><body><table><tr><th>id</th><th>message</th></tr>");
    for fortune in fortunes {
        let _ = write!(out, "<tr><td>{}</td><td>", fortune.id);
        for ch in fortune.message.chars() {
            match ch {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#x27;"),
                other => out.push(other),
            }
        }
        out.push_str("</td></tr>");
    }
    out.push_str("</table></body></html>");
    out
}

fn counts() -> (u64, u64, u64) {
    (ALLOCS.load(Ordering::Relaxed), REALLOCS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}

fn measure(name: &str, mut render: impl FnMut() -> usize) {
    let length = render();
    let before = counts();
    let _ = render();
    let after = counts();
    let mut samples = Vec::new();
    for _ in 0..9 {
        let started = Instant::now();
        let mut sink = 0;
        for _ in 0..20_000 {
            sink += render();
        }
        std::hint::black_box(sink);
        samples.push(started.elapsed().as_nanos() as f64 / 20_000.0);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    println!(
        "{name}: {} bytes; per render: {} allocs + {} reallocs, {} bytes requested; {:.0} ns (median of 9 x 20000; min {:.0}, max {:.0})",
        length,
        after.0 - before.0,
        after.1 - before.1,
        after.2 - before.2,
        samples[4],
        samples[0],
        samples[8]
    );
}

fn main() {
    let rows = rows();
    let from_html = render_html(&rows).into_string();
    let from_askama = render_askama(&rows);
    let from_plain = render_plain(&rows);
    println!("same output: html!==askama {} ; html!==plain {}", from_html == from_askama, from_html == from_plain);
    if from_html != from_askama {
        println!("html!   {from_html}\naskama  {from_askama}");
    }
    measure("html! (frameworks/rullst)", || render_html(&rows).into_string().len());
    measure("askama (frameworks/axum)", || render_askama(&rows).len());
    measure("plain String (reference)", || render_plain(&rows).len());
    measure("row mapping + sort (13 Strings)", || self::rows().len());
}
