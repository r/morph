fn main() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (y, m, d) = epoch_to_ymd(now);
    println!("cargo:rustc-env=MORPH_BUILD_DATE={y:04}-{m:02}-{d:02}");
}

fn epoch_to_ymd(secs: u64) -> (u64, u64, u64) {
    let days = secs / 86400;
    let mut y = 1970;
    let mut rem = days;
    loop {
        let ylen = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if rem < ylen { break; }
        rem -= ylen;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    for md in mdays {
        if rem < md { break; }
        rem -= md;
        m += 1;
    }
    (y, m + 1, rem + 1)
}
