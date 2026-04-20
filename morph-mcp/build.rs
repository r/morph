fn main() {
    println!("cargo:rerun-if-changed=_always_rebuild");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    println!("cargo:rustc-env=MORPH_BUILD_DATE={}", epoch_to_iso(now));
}

fn epoch_to_iso(secs: u64) -> String {
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hh = day_secs / 3600;
    let mm = (day_secs % 3600) / 60;
    let ss = day_secs % 60;

    let mut y: u64 = 1970;
    let mut rem = days;
    loop {
        let ylen = if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) { 366 } else { 365 };
        if rem < ylen { break; }
        rem -= ylen;
        y += 1;
    }
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 0u64;
    for md in mdays {
        if rem < md { break; }
        rem -= md;
        mo += 1;
    }
    format!("{y:04}-{:02}-{:02}T{hh:02}:{mm:02}:{ss:02}Z", mo + 1, rem + 1)
}
