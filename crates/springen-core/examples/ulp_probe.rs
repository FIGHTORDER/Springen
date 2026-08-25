// measure max ulp gap between vendored fdlibm and the host libm
fn ulps(a: f64, b: f64) -> i64 {
    (a.to_bits() as i64 - b.to_bits() as i64).abs()
}
fn main() {
    let (mut s, mut c, mut at, mut lg, mut pw) = (0i64, 0i64, 0i64, 0i64, 0i64);
    for i in 0..2000 {
        let x = (i as f64 - 1000.0) * 0.0137;
        s = s.max(ulps(springen_core::fdlibm::sin(x), x.sin()));
        c = c.max(ulps(springen_core::fdlibm::cos(x), x.cos()));
        at = at.max(ulps(springen_core::fdlibm::atan(x), x.atan()));
        if x > 0.0 {
            lg = lg.max(ulps(springen_core::fdlibm::log(x), x.ln()));
            pw = pw.max(ulps(springen_core::fdlibm::pow(x, 2.5), x.powf(2.5)));
        }
    }
    println!("max ulp gap vs host libm: sin={s} cos={c} atan={at} log={lg} pow={pw}");
}
