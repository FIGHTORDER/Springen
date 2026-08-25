//! Deterministic transcendentals.
//!
//! The prototype is the correctness oracle and it ran on V8, whose `Math.sin`,
//! `Math.cos`, `Math.tan`, `Math.atan`, `Math.log` and `Math.pow` are FDLIBM
//! (Sun Microsystems) ports. Neither glibc nor musl reproduces them exactly —
//! measured over the 1024 gradient angles the noise field uses, glibc differs
//! on 75 of 2048 results and musl on 21, always by one unit in the last place.
//!
//! One ulp is enough to move a 16-bit heightmap sample by one LSB, which
//! changes the baked PNG's hash, so "close enough" is not a option if the
//! golden files are to mean anything. The same argument applies across targets:
//! a Windows build linked against MSVC's libm would disagree with a Linux one.
//! Vendoring the kernel removes the platform from the equation entirely.
//!
//! Everything here is a straight transcription of FDLIBM 5.3. `sqrt`, `floor`,
//! `abs` and `round` are exact under IEEE-754 and stay on `std`.
//!
//! The lints below are disabled for the module rather than worked around: the
//! constants are FDLIBM's to the last digit, and `x - x` is how it propagates
//! NaN and signed zero. Any "tidier" rewrite is a behaviour change, and this
//! file only has value if it matches the original bit for bit.
#![allow(
    clippy::eq_op,
    clippy::excessive_precision,
    clippy::approx_constant,
    clippy::needless_range_loop,
    clippy::int_plus_one,
    clippy::unnecessary_cast,
    clippy::explicit_counter_loop
)]

#[inline]
fn hi(x: f64) -> u32 {
    (x.to_bits() >> 32) as u32
}
#[inline]
fn lo(x: f64) -> u32 {
    x.to_bits() as u32
}
#[inline]
fn from_words(h: u32, l: u32) -> f64 {
    f64::from_bits((u64::from(h) << 32) | u64::from(l))
}
#[inline]
fn with_hi(x: f64, h: u32) -> f64 {
    from_words(h, lo(x))
}
#[inline]
fn with_lo_zero(x: f64) -> f64 {
    from_words(hi(x), 0)
}

/* -------------------------------------------------------------- sin / cos */

const S1: f64 = -1.666_666_666_666_663_2e-1;
const S2: f64 = 8.333_333_333_322_489e-3;
const S3: f64 = -1.984_126_982_985_795e-4;
const S4: f64 = 2.755_731_370_707_007e-6;
const S5: f64 = -2.505_076_025_340_686e-8;
const S6: f64 = 1.589_690_995_211_55e-10;

fn kernel_sin(x: f64, y: f64, iy: i32) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix < 0x3e40_0000 && (x as i32) == 0 {
        return x;
    }
    let z = x * x;
    let v = z * x;
    let r = S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)));
    if iy == 0 {
        x + v * (S1 + z * r)
    } else {
        x - ((z * (0.5 * y - v * r) - y) - v * S1)
    }
}

const C1: f64 = 4.166_666_666_666_660_2e-2;
const C2: f64 = -1.388_888_888_887_411e-3;
const C3: f64 = 2.480_158_728_947_673e-5;
const C4: f64 = -2.755_731_435_139_066e-7;
const C5: f64 = 2.087_572_321_298_175e-9;
const C6: f64 = -1.135_964_755_778_819_5e-11;

fn kernel_cos(x: f64, y: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix < 0x3e40_0000 && (x as i32) == 0 {
        return 1.0;
    }
    let z = x * x;
    let r = z * (C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6)))));
    if ix < 0x3FD3_3333 {
        1.0 - (0.5 * z - (z * r - x * y))
    } else {
        let qx = if ix > 0x3fe9_0000 {
            0.28125
        } else {
            from_words(ix - 0x0020_0000, 0)
        };
        let hz = 0.5 * z - qx;
        let a = 1.0 - qx;
        a - (hz - (z * r - x * y))
    }
}

const INVPIO2: f64 = 6.366_197_723_675_814e-1;
const PIO2_1: f64 = 1.570_796_326_734_125_6;
const PIO2_1T: f64 = 6.077_100_506_506_192e-11;
const PIO2_2: f64 = 6.077_100_506_303_966e-11;
const PIO2_2T: f64 = 2.022_266_248_795_950_7e-21;
const PIO2_3: f64 = 2.022_266_248_711_166_5e-21;
const PIO2_3T: f64 = 8.478_427_660_368_9e-32;
const TWO24: f64 = 1.677_721_6e7;
/// 2^-24, exactly.
const TWON24: f64 = 5.960_464_477_539_062_5e-8;
/// 2^1000 and 2^-1000, for `scalbn`'s coarse steps.
const TWO1000: f64 = 1.071_508_607_186_267_3e301;
const TWON1000: f64 = 9.332_636_185_032_189e-302;

const NPIO2_HW: [u32; 32] = [
    0x3FF921FB, 0x400921FB, 0x4012D97C, 0x401921FB, 0x401F6A7A, 0x4022D97C, 0x4025FDBB, 0x402921FB,
    0x402C463A, 0x402F6A7A, 0x4031475C, 0x4032D97C, 0x40346B9C, 0x4035FDBB, 0x40378FDB, 0x403921FB,
    0x403AB41B, 0x403C463A, 0x403DD85A, 0x403F6A7A, 0x40407E4C, 0x4041475C, 0x4042106C, 0x4042D97C,
    0x4043A28C, 0x40446B9C, 0x404534AC, 0x4045FDBB, 0x4046C6CB, 0x40478FDB, 0x404858EB, 0x404921FB,
];

/// 396 hex digits of 2/pi, as FDLIBM stores them: 24 bits per entry.
const TWO_OVER_PI: [i32; 66] = [
    0xA2F983, 0x6E4E44, 0x1529FC, 0x2757D1, 0xF534DD, 0xC0DB62, 0x95993C, 0x439041, 0xFE5163,
    0xABDEBB, 0xC561B7, 0x246E3A, 0x424DD2, 0xE00649, 0x2EEA09, 0xD1921C, 0xFE1DEB, 0x1CB129,
    0xA73EE8, 0x8235F5, 0x2EBB44, 0x84E99C, 0x7026B4, 0x5F7E41, 0x3991D6, 0x398353, 0x39F49C,
    0x845F8B, 0xBDF928, 0x3B1FF8, 0x97FFDE, 0x05980F, 0xEF2F11, 0x8B5A0A, 0x6D1F6D, 0x367ECF,
    0x27CB09, 0xB74F46, 0x3F669E, 0x5FEA2D, 0x7527BA, 0xC7EBE5, 0xF17B3D, 0x0739F7, 0x8A5292,
    0xEA6BFB, 0x5FB11F, 0x8D5D08, 0x560330, 0x46FC7B, 0x6BABF0, 0xCFBC20, 0x9AF436, 0x1DA9E3,
    0x91615E, 0xE61B08, 0x659985, 0x5F14A0, 0x68408D, 0xFFD880, 0x4D7327, 0x310606, 0x1556CA,
    0x73A8C9, 0x60E27B, 0xC08C6B,
];

const PIO2_TAB: [f64; 8] = [
    1.570_796_310_901_641_8,
    1.589_325_477_982_183_9e-8,
    1.984_187_622_756_997_7e-16,
    1.404_566_431_050_057e-24,
    1.155_998_099_154_400_7e-32,
    1.212_022_133_837_281_5e-40,
    1.760_255_720_886_078_5e-48,
    2.167_622_538_811_580_2e-56,
];

/// FDLIBM `__kernel_rem_pio2`, restricted to the double-precision case
/// (`prec == 2`) which is the only one `rem_pio2` ever asks for.
fn kernel_rem_pio2(x: &[f64], y: &mut [f64; 2], e0: i32, nx: usize) -> i32 {
    let mut iq = [0i32; 20];
    let mut f = [0.0f64; 20];
    let mut fq = [0.0f64; 20];
    let mut q = [0.0f64; 20];

    // initialize jk, jz, jx, jv
    let jk = 4usize; // prec == 2
    let mut jz = jk;
    let jx = nx - 1;
    let mut jv = (e0 - 3) / 24;
    if jv < 0 {
        jv = 0;
    }
    let mut q0 = e0 - 24 * (jv + 1);

    // set up f[0..jx+jk] where f[i] = TWO_OVER_PI[jv+i]
    let j0 = jv - jx as i32;
    let m = jx + jk;
    let mut j = j0;
    for fi in f.iter_mut().take(m + 1) {
        *fi = if j < 0 {
            0.0
        } else {
            f64::from(TWO_OVER_PI[j as usize])
        };
        j += 1;
    }

    // compute q[0..jk] = x[0..jx] * f[jk..jk+jx]
    for i in 0..=jk {
        let mut fw = 0.0;
        for (j, xj) in x.iter().enumerate().take(jx + 1) {
            fw += xj * f[jx + i - j];
        }
        q[i] = fw;
    }

    let mut n;
    let mut ih;
    let mut z;
    'recompute: loop {
        // distill q[] into iq[] reversingly
        let mut i = 0i32;
        z = q[jz];
        let mut j = jz;
        while j > 0 {
            let fw = ((TWON24 * z) as i32) as f64;
            iq[i as usize] = (z - TWO24 * fw) as i32;
            z = q[j - 1] + fw;
            i += 1;
            j -= 1;
        }

        // compute n
        z = scalbn(z, q0);
        z -= 8.0 * (z * 0.125).floor(); // trim off integer >= 8
        n = z as i32;
        z -= f64::from(n);
        ih = 0;
        if q0 > 0 {
            // need iq[jz-1] to determine n
            let i = iq[jz - 1] >> (24 - q0);
            n += i;
            iq[jz - 1] -= i << (24 - q0);
            ih = iq[jz - 1] >> (23 - q0);
        } else if q0 == 0 {
            ih = iq[jz - 1] >> 23;
        } else if z >= 0.5 {
            ih = 2;
        }

        if ih > 0 {
            // q > 0.5
            n += 1;
            let mut carry = 0;
            for iqi in iq.iter_mut().take(jz) {
                // compute 1 - q
                let j = *iqi;
                if carry == 0 {
                    if j != 0 {
                        carry = 1;
                        *iqi = 0x1000000 - j;
                    }
                } else {
                    *iqi = 0xffffff - j;
                }
            }
            if q0 > 0 {
                // rare case: chance is 1 in 12
                match q0 {
                    1 => iq[jz - 1] &= 0x7fffff,
                    2 => iq[jz - 1] &= 0x3fffff,
                    _ => {}
                }
            }
            if ih == 2 {
                z = 1.0 - z;
                if carry != 0 {
                    z -= scalbn(1.0, q0);
                }
            }
        }

        // check if recomputation is needed
        if z == 0.0 {
            let mut j = 0;
            for i in (jk..=jz - 1).rev() {
                j |= iq[i];
            }
            if j == 0 {
                // need recomputation
                let mut k = 1;
                while jk >= k + 1 && iq[jk - k] == 0 {
                    k += 1; // k = no. of terms needed
                }
                for i in (jz + 1)..=(jz + k) {
                    // add q[jz+1] to q[jz+k]
                    f[jx + i] = f64::from(TWO_OVER_PI[(jv as usize) + i]);
                    let mut fw = 0.0;
                    for (j, xj) in x.iter().enumerate().take(jx + 1) {
                        fw += xj * f[jx + i - j];
                    }
                    q[i] = fw;
                }
                jz += k;
                continue 'recompute;
            }
        }
        break;
    }

    // chop off zero terms
    if z == 0.0 {
        jz -= 1;
        q0 -= 24;
        while jz > 0 && iq[jz] == 0 {
            jz -= 1;
            q0 -= 24;
        }
    } else {
        // break z into 24-bit if necessary
        z = scalbn(z, -q0);
        if z >= TWO24 {
            let fw = ((TWON24 * z) as i32) as f64;
            iq[jz] = (z - TWO24 * fw) as i32;
            jz += 1;
            q0 += 24;
            iq[jz] = fw as i32;
        } else {
            iq[jz] = z as i32;
        }
    }

    // convert integer "bit" chunk to floating-point value
    let mut fw = scalbn(1.0, q0);
    for i in (0..=jz).rev() {
        q[i] = fw * f64::from(iq[i]);
        fw *= TWON24;
    }

    // compute PIo2[0,...,jp]*q[jz,...,0]
    for i in (0..=jz).rev() {
        let mut fw = 0.0;
        let mut k = 0;
        while k <= jk && k <= jz - i {
            fw += PIO2_TAB[k] * q[i + k];
            k += 1;
        }
        fq[jz - i] = fw;
    }

    // compress fq[] into y[] (prec == 2)
    let mut fw = 0.0;
    for v in fq.iter().take(jz + 1) {
        fw += v;
    }
    y[0] = if ih == 0 { fw } else { -fw };
    fw = fq[0] - fw;
    for v in fq.iter().take(jz + 1).skip(1) {
        fw += v;
    }
    y[1] = if ih == 0 { fw } else { -fw };
    n & 7
}

fn scalbn(x: f64, n: i32) -> f64 {
    // Sufficient for the ranges FDLIBM's callers use here.
    let mut y = x;
    let mut n = n;
    while n > 1000 {
        y *= TWO1000;
        n -= 1000;
    }
    while n < -1000 {
        y *= TWON1000;
        n += 1000;
    }
    y * f64::from_bits(((0x3ff_i64 + i64::from(n)) as u64) << 52)
}

fn rem_pio2(x: f64, y: &mut [f64; 2]) -> i32 {
    let hx = hi(x) as i32;
    let ix = (hx & 0x7fff_ffff) as u32;

    if ix <= 0x3fe9_21fb {
        // |x| <= pi/4, no reduction needed
        y[0] = x;
        y[1] = 0.0;
        return 0;
    }
    if ix < 0x4002_d97c {
        // |x| < 3pi/4, special case with n = +-1
        if hx > 0 {
            let mut z = x - PIO2_1;
            if ix != 0x3ff9_21fb {
                y[0] = z - PIO2_1T;
                y[1] = (z - y[0]) - PIO2_1T;
            } else {
                z -= PIO2_2;
                y[0] = z - PIO2_2T;
                y[1] = (z - y[0]) - PIO2_2T;
            }
            return 1;
        }
        let mut z = x + PIO2_1;
        if ix != 0x3ff9_21fb {
            y[0] = z + PIO2_1T;
            y[1] = (z - y[0]) + PIO2_1T;
        } else {
            z += PIO2_2;
            y[0] = z + PIO2_2T;
            y[1] = (z - y[0]) + PIO2_2T;
        }
        return -1;
    }
    if ix <= 0x4139_21fb {
        // |x| ~<= 2^19 pi/2, medium size
        let t = x.abs();
        let n = (t * INVPIO2 + 0.5) as i32;
        let fnn = f64::from(n);
        let mut r = t - fnn * PIO2_1;
        let mut w = fnn * PIO2_1T;
        if n < 32 && ix != NPIO2_HW[(n - 1) as usize] {
            y[0] = r - w;
        } else {
            let j = ix >> 20;
            y[0] = r - w;
            let high = hi(y[0]);
            let i = (j as i32) - (((high >> 20) & 0x7ff) as i32);
            if i > 16 {
                // 2nd iteration needed, good to 118 bits
                let t2 = r;
                w = fnn * PIO2_2;
                r = t2 - w;
                w = fnn * PIO2_2T - ((t2 - r) - w);
                y[0] = r - w;
                let high = hi(y[0]);
                let i2 = (j as i32) - (((high >> 20) & 0x7ff) as i32);
                if i2 > 49 {
                    // 3rd iteration, 151 bits accuracy
                    let t3 = r;
                    w = fnn * PIO2_3;
                    r = t3 - w;
                    w = fnn * PIO2_3T - ((t3 - r) - w);
                    y[0] = r - w;
                }
            }
        }
        y[1] = (r - y[0]) - w;
        return if hx < 0 {
            y[0] = -y[0];
            y[1] = -y[1];
            -n
        } else {
            n
        };
    }
    // all other (large) arguments
    if ix >= 0x7ff0_0000 {
        y[0] = x - x;
        y[1] = y[0];
        return 0;
    }
    // set z = scalbn(|x|, ilogb(x) - 23)
    let e0 = ((ix >> 20) as i32) - 1046; // e0 = ilogb(z) - 23
    let mut z = from_words(ix - ((e0 as u32) << 20), lo(x));
    let mut tx = [0.0f64; 3];
    for t in tx.iter_mut().take(2) {
        *t = (z as i32) as f64;
        z = (z - *t) * TWO24;
    }
    tx[2] = z;
    let mut nx = 3usize;
    while nx > 1 && tx[nx - 1] == 0.0 {
        nx -= 1;
    }
    let n = kernel_rem_pio2(&tx[..nx], y, e0, nx);
    if hx < 0 {
        y[0] = -y[0];
        y[1] = -y[1];
        return -n;
    }
    n
}

pub fn sin(x: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix <= 0x3fe9_21fb {
        return kernel_sin(x, 0.0, 0);
    }
    if ix >= 0x7ff0_0000 {
        return x - x;
    }
    let mut y = [0.0f64; 2];
    let n = rem_pio2(x, &mut y);
    match n & 3 {
        0 => kernel_sin(y[0], y[1], 1),
        1 => kernel_cos(y[0], y[1]),
        2 => -kernel_sin(y[0], y[1], 1),
        _ => -kernel_cos(y[0], y[1]),
    }
}

pub fn cos(x: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix <= 0x3fe9_21fb {
        return kernel_cos(x, 0.0);
    }
    if ix >= 0x7ff0_0000 {
        return x - x;
    }
    let mut y = [0.0f64; 2];
    let n = rem_pio2(x, &mut y);
    match n & 3 {
        0 => kernel_cos(y[0], y[1]),
        1 => -kernel_sin(y[0], y[1], 1),
        2 => -kernel_cos(y[0], y[1]),
        _ => kernel_sin(y[0], y[1], 1),
    }
}

/* -------------------------------------------------------------------- tan */

const T: [f64; 13] = [
    3.333_333_333_333_341e-1,
    1.333_333_333_332_012_4e-1,
    5.396_825_397_622_605e-2,
    2.186_948_829_485_954_2e-2,
    8.863_239_823_599_3e-3,
    3.592_079_107_591_312_5e-3,
    1.456_209_454_325_290_3e-3,
    5.880_412_408_202_641e-4,
    2.464_631_348_184_699e-4,
    7.817_944_429_395_571e-5,
    7.140_724_913_826_082e-5,
    -1.855_863_748_552_754_6e-5,
    2.590_730_518_636_337e-5,
];
const PIO4: f64 = 7.853_981_633_974_483e-1;
const PIO4LO: f64 = 3.061_616_997_868_383e-17;

fn kernel_tan(x: f64, y: f64, iy: i32) -> f64 {
    let hx = hi(x) as i32;
    let ix = (hx & 0x7fff_ffff) as u32;
    let mut x = x;
    let mut y = y;
    if ix < 0x3e30_0000 && (x as i32) == 0 {
        // x < 2**-28
        if ((ix | lo(x)) as i32 | (iy + 1)) == 0 {
            return 1.0 / x.abs();
        }
        if iy == 1 {
            return x;
        }
        // compute -1/(x+y) carefully
        let w = x + y;
        let z = with_lo_zero(w);
        let v = y - (z - x);
        let a = -1.0 / w;
        let t = with_lo_zero(a);
        let s = 1.0 + t * z;
        return t + a * (s + t * v);
    }
    if ix >= 0x3FE5_9428 {
        // |x| >= 0.6744
        if hx < 0 {
            x = -x;
            y = -y;
        }
        let z = PIO4 - x;
        let w = PIO4LO - y;
        x = z + w;
        y = 0.0;
    }
    let z = x * x;
    let w = z * z;
    let mut r = T[1] + w * (T[3] + w * (T[5] + w * (T[7] + w * (T[9] + w * T[11]))));
    let v = z * (T[2] + w * (T[4] + w * (T[6] + w * (T[8] + w * (T[10] + w * T[12])))));
    let s = z * x;
    r = y + z * (s * (r + v) + y);
    r += T[0] * s;
    let w = x + r;
    if ix >= 0x3FE5_9428 {
        let v = f64::from(iy);
        return f64::from(1 - ((hx >> 30) & 2)) * (v - 2.0 * (x - (w * w / (w + v) - r)));
    }
    if iy == 1 {
        return w;
    }
    let z = with_lo_zero(w);
    let v = r - (z - x);
    let a = -1.0 / w;
    let t = with_lo_zero(a);
    let s = 1.0 + t * z;
    t + a * (s + t * v)
}

pub fn tan(x: f64) -> f64 {
    let ix = hi(x) & 0x7fff_ffff;
    if ix <= 0x3fe9_21fb {
        return kernel_tan(x, 0.0, 1);
    }
    if ix >= 0x7ff0_0000 {
        return x - x;
    }
    let mut y = [0.0f64; 2];
    let n = rem_pio2(x, &mut y);
    kernel_tan(y[0], y[1], 1 - ((n & 1) << 1))
}

/* ------------------------------------------------------------------- atan */

const ATANHI: [f64; 4] = [
    4.636_476_090_008_061e-1,
    7.853_981_633_974_483e-1,
    9.827_937_232_473_291e-1,
    1.570_796_326_794_896_6,
];
const ATANLO: [f64; 4] = [
    2.269_877_745_296_168_7e-17,
    3.061_616_997_868_383e-17,
    1.390_331_103_123_099_8e-17,
    6.123_233_995_736_766e-17,
];
const AT: [f64; 11] = [
    3.333_333_333_333_293_2e-1,
    -1.999_999_999_987_648_3e-1,
    1.428_571_427_250_346_6e-1,
    -1.111_111_040_546_235_6e-1,
    9.090_887_133_436_507e-2,
    -7.691_876_205_044_83e-2,
    6.661_073_137_387_531e-2,
    -5.833_570_133_790_573_5e-2,
    4.976_877_994_615_932e-2,
    -3.653_157_274_421_691_6e-2,
    1.628_582_011_536_578_2e-2,
];

pub fn atan(x: f64) -> f64 {
    let hx = hi(x) as i32;
    let ix = (hx & 0x7fff_ffff) as u32;
    let mut x = x;
    let id: i32;
    if ix >= 0x4410_0000 {
        // |x| >= 2^66
        if ix > 0x7ff0_0000 || (ix == 0x7ff0_0000 && lo(x) != 0) {
            return x + x;
        }
        return if hx > 0 {
            ATANHI[3] + ATANLO[3]
        } else {
            -ATANHI[3] - ATANLO[3]
        };
    }
    if ix < 0x3fdc_0000 {
        // |x| < 0.4375
        if ix < 0x3e20_0000 {
            // |x| < 2^-29
            return x;
        }
        id = -1;
    } else {
        x = x.abs();
        if ix < 0x3ff3_0000 {
            // |x| < 1.1875
            if ix < 0x3fe6_0000 {
                // 7/16 <= |x| < 11/16
                id = 0;
                x = (2.0 * x - 1.0) / (2.0 + x);
            } else {
                // 11/16 <= |x| < 19/16
                id = 1;
                x = (x - 1.0) / (x + 1.0);
            }
        } else if ix < 0x4003_8000 {
            // |x| < 2.4375
            id = 2;
            x = (x - 1.5) / (1.0 + 1.5 * x);
        } else {
            // 2.4375 <= |x| < 2^66
            id = 3;
            x = -1.0 / x;
        }
    }
    let z = x * x;
    let w = z * z;
    let s1 = z * (AT[0] + w * (AT[2] + w * (AT[4] + w * (AT[6] + w * (AT[8] + w * AT[10])))));
    let s2 = w * (AT[1] + w * (AT[3] + w * (AT[5] + w * (AT[7] + w * AT[9]))));
    if id < 0 {
        return x - x * (s1 + s2);
    }
    let z = ATANHI[id as usize] - ((x * (s1 + s2) - ATANLO[id as usize]) - x);
    if hx < 0 {
        -z
    } else {
        z
    }
}

/* -------------------------------------------------------------------- log */

const LN2_HI: f64 = 6.931_471_803_691_238e-1;
const LN2_LO: f64 = 1.908_214_929_270_587_7e-10;
const LG1: f64 = 6.666_666_666_666_735e-1;
const LG2: f64 = 3.999_999_999_940_942e-1;
const LG3: f64 = 2.857_142_874_366_239e-1;
const LG4: f64 = 2.222_219_843_214_978_4e-1;
const LG5: f64 = 1.818_357_216_161_805e-1;
const LG6: f64 = 1.531_383_769_920_937_3e-1;
const LG7: f64 = 1.479_819_860_511_658_6e-1;

pub fn log(x: f64) -> f64 {
    let mut x = x;
    let mut hx = hi(x) as i32;
    let lx = lo(x);
    let mut k = 0i32;

    if hx < 0x0010_0000 {
        // x < 2**-1022
        if (((hx & 0x7fff_ffff) as u32) | lx) == 0 {
            return f64::NEG_INFINITY;
        }
        if hx < 0 {
            return f64::NAN;
        }
        k -= 54;
        x *= 1.801_439_850_948_198_4e16;
        hx = hi(x) as i32;
    }
    if hx >= 0x7ff0_0000 {
        return x + x;
    }
    k += (hx >> 20) - 1023;
    hx &= 0x000f_ffff;
    let i = (hx + 0x95f64) & 0x100000;
    x = with_hi(x, (hx | (i ^ 0x3ff0_0000)) as u32); // normalize x or x/2
    k += i >> 20;
    let f = x - 1.0;
    let dk = f64::from(k);

    if (0x000f_ffff & (2 + hx)) < 3 {
        // |f| < 2**-20
        if f == 0.0 {
            if k == 0 {
                return 0.0;
            }
            return dk * LN2_HI + dk * LN2_LO;
        }
        let r = f * f * (0.5 - 0.333_333_333_333_333_33 * f);
        if k == 0 {
            return f - r;
        }
        return dk * LN2_HI - ((r - dk * LN2_LO) - f);
    }
    let s = f / (2.0 + f);
    let z = s * s;
    let mut i = hx - 0x6147a;
    let w = z * z;
    let j = 0x6b851 - hx;
    let t1 = w * (LG2 + w * (LG4 + w * LG6));
    let t2 = z * (LG1 + w * (LG3 + w * (LG5 + w * LG7)));
    i |= j;
    let r = t2 + t1;
    if i > 0 {
        let hfsq = 0.5 * f * f;
        if k == 0 {
            f - (hfsq - s * (hfsq + r))
        } else {
            dk * LN2_HI - ((hfsq - (s * (hfsq + r) + dk * LN2_LO)) - f)
        }
    } else if k == 0 {
        f - s * (f - r)
    } else {
        dk * LN2_HI - ((s * (f - r) - dk * LN2_LO) - f)
    }
}

/* -------------------------------------------------------------------- pow */

const BP: [f64; 2] = [1.0, 1.5];
const DP_H: [f64; 2] = [0.0, 5.849_624_872_207_642e-1];
const DP_L: [f64; 2] = [0.0, 1.350_039_202_129_749e-8];
const TWO53: f64 = 9_007_199_254_740_992.0;
const POW_HUGE: f64 = 1.0e300;
const POW_TINY: f64 = 1.0e-300;
const PL1: f64 = 5.999_999_999_999_946_5e-1;
const PL2: f64 = 4.285_714_285_785_502e-1;
const PL3: f64 = 3.333_333_298_183_774_3e-1;
const PL4: f64 = 2.727_281_238_085_340_1e-1;
const PL5: f64 = 2.306_607_457_755_617_5e-1;
const PL6: f64 = 2.069_750_178_003_384_2e-1;
const P1: f64 = 1.666_666_666_666_660_2e-1;
const P2: f64 = -2.777_777_777_701_559_3e-3;
const P3: f64 = 6.613_756_321_437_934e-5;
const P4: f64 = -1.653_390_220_546_525_2e-6;
const P5: f64 = 4.138_136_797_057_238_5e-8;
const LG2C: f64 = 6.931_471_805_599_453e-1;
const LG2_H: f64 = 6.931_471_824_645_996e-1;
const LG2_L: f64 = -1.904_654_299_957_768e-9;
const OVT: f64 = 8.008_566_259_537_294e-17;
const CP: f64 = 9.617_966_939_259_756e-1;
const CP_H: f64 = 9.617_967_009_544_373e-1;
const CP_L: f64 = -7.028_461_650_952_758e-9;
const IVLN2: f64 = 1.442_695_040_888_963_4;
const IVLN2_H: f64 = 1.442_695_021_629_333_5;
const IVLN2_L: f64 = 1.925_962_991_126_617_4e-8;

// `t1` and `t2` are declared here and set in one of two branches fifty lines
// apart, which clippy would rather were a single initialiser. Not here: this
// is a transcription of FDLIBM's `__ieee754_pow`, and its worth is computing
// what V8 computes, bit for bit. Rewriting the control flow to please a style
// lint is how a port stops matching the thing it is a port of - and one ulp
// here moves a heightmap sample, which changes a baked PNG's hash, which is
// what the golden files exist to catch.
#[allow(clippy::needless_late_init)]
pub fn pow(x: f64, y: f64) -> f64 {
    let hx = hi(x) as i32;
    let lx = lo(x);
    let hy = hi(y) as i32;
    let ly = lo(y);
    let mut ix = hx & 0x7fff_ffff;
    let iy = hy & 0x7fff_ffff;

    // y == zero: x**0 = 1
    if (iy as u32 | ly) == 0 {
        return 1.0;
    }
    // +-NaN return x+y
    if ix > 0x7ff0_0000
        || (ix == 0x7ff0_0000 && lx != 0)
        || iy > 0x7ff0_0000
        || (iy == 0x7ff0_0000 && ly != 0)
    {
        return x + y;
    }

    // yisint: 0 = not an integer, 1 = odd integer, 2 = even integer
    let mut yisint = 0i32;
    if hx < 0 {
        if iy >= 0x4340_0000 {
            yisint = 2;
        } else if iy >= 0x3ff0_0000 {
            let k = (iy >> 20) - 0x3ff;
            if k > 20 {
                let j = ly >> (52 - k);
                if (j << (52 - k)) == ly {
                    yisint = 2 - ((j & 1) as i32);
                }
            } else if ly == 0 {
                let j = iy >> (20 - k);
                if (j << (20 - k)) == iy {
                    yisint = 2 - (j & 1);
                }
            }
        }
    }

    // special value of y
    if ly == 0 {
        if iy == 0x7ff0_0000 {
            // y is +-inf
            if ((ix - 0x3ff0_0000) as u32 | lx) == 0 {
                return y - y; // inf**+-1 is NaN
            } else if ix >= 0x3ff0_0000 {
                return if hy >= 0 { y } else { 0.0 };
            } else {
                return if hy < 0 { -y } else { 0.0 };
            }
        }
        if iy == 0x3ff0_0000 {
            return if hy < 0 { 1.0 / x } else { x };
        }
        if hy == 0x4000_0000 {
            return x * x;
        }
        if hy == 0x3fe0_0000 && hx >= 0 {
            return x.sqrt();
        }
    }

    let mut ax = x.abs();
    // special value of x
    if lx == 0 && (ix == 0x7ff0_0000 || ix == 0 || ix == 0x3ff0_0000) {
        let mut z = ax;
        if hy < 0 {
            z = 1.0 / z;
        }
        if hx < 0 {
            if ((ix - 0x3ff0_0000) | yisint) == 0 {
                return f64::NAN; // (-1)**non-int
            } else if yisint == 1 {
                z = -z;
            }
        }
        return z;
    }

    let mut n = (hx >> 31) + 1;
    // (x<0)**(non-int) is NaN
    if (n | yisint) == 0 {
        return f64::NAN;
    }
    let mut s = 1.0f64;
    if (n | (yisint - 1)) == 0 {
        s = -1.0; // (-ve)**(odd int)
    }

    let t1: f64;
    let t2: f64;
    if iy > 0x41e0_0000 {
        // |y| > 2**31
        if iy > 0x43f0_0000 {
            // |y| > 2**64, must over/underflow
            if ix <= 0x3fef_ffff {
                return if hy < 0 {
                    POW_HUGE * POW_HUGE
                } else {
                    POW_TINY * POW_TINY
                };
            }
            if ix >= 0x3ff0_0000 {
                return if hy > 0 {
                    POW_HUGE * POW_HUGE
                } else {
                    POW_TINY * POW_TINY
                };
            }
        }
        if ix < 0x3fef_ffff {
            return if hy < 0 {
                s * POW_HUGE * POW_HUGE
            } else {
                s * POW_TINY * POW_TINY
            };
        }
        if ix > 0x3ff0_0000 {
            return if hy > 0 {
                s * POW_HUGE * POW_HUGE
            } else {
                s * POW_TINY * POW_TINY
            };
        }
        // |1-x| is tiny <= 2**-20: log(x) ~ x - x^2/2 + x^3/3 - x^4/4
        let t = ax - 1.0;
        let w = (t * t) * (0.5 - t * (0.333_333_333_333_333_33 - t * 0.25));
        let u = IVLN2_H * t;
        let v = t * IVLN2_L - w * IVLN2;
        let tt1 = with_lo_zero(u + v);
        t1 = tt1;
        t2 = v - (t1 - u);
    } else {
        n = 0;
        // take care of subnormal number
        if ix < 0x0010_0000 {
            ax *= TWO53;
            n -= 53;
            ix = hi(ax) as i32;
        }
        n += (ix >> 20) - 0x3ff;
        let j = ix & 0x000f_ffff;
        // determine interval
        ix = j | 0x3ff0_0000; // normalize ix
        let k: usize;
        if j <= 0x3988E {
            k = 0; // |x| < sqrt(3/2)
        } else if j < 0xBB67A {
            k = 1; // |x| < sqrt(3)
        } else {
            k = 0;
            n += 1;
            ix -= 0x0010_0000;
        }
        ax = with_hi(ax, ix as u32);

        // compute ss = s_h + s_l = (x-1)/(x+1) or (x-1.5)/(x+1.5)
        let u = ax - BP[k];
        let v = 1.0 / (ax + BP[k]);
        let ss = u * v;
        let s_h = with_lo_zero(ss);
        // t_h = ax + bp[k] high
        let t_h = from_words(
            ((((ix as u32) >> 1) | 0x2000_0000) + 0x0008_0000 + ((k as u32) << 18)) as u32,
            0,
        );
        let t_l = ax - (t_h - BP[k]);
        let s_l = v * ((u - s_h * t_h) - s_h * t_l);
        // compute log(ax)
        let mut s2 = ss * ss;
        let mut r = s2 * s2 * (PL1 + s2 * (PL2 + s2 * (PL3 + s2 * (PL4 + s2 * (PL5 + s2 * PL6)))));
        r += s_l * (s_h + ss);
        s2 = s_h * s_h;
        let t_h = with_lo_zero(3.0 + s2 + r);
        let t_l = r - ((t_h - 3.0) - s2);
        // u + v = ss * (1 + ...)
        let u = s_h * t_h;
        let v = s_l * t_h + t_l * ss;
        // 2/(3log2) * (ss + ...)
        let p_h = with_lo_zero(u + v);
        let p_l = v - (p_h - u);
        let z_h = CP_H * p_h;
        let z_l = CP_L * p_h + p_l * CP + DP_L[k];
        // log2(ax) = (ss + ..) * 2/(3*log2) = n + dp_h + z_h + z_l
        let t = f64::from(n);
        t1 = with_lo_zero((z_h + z_l) + DP_H[k] + t);
        t2 = z_l - (((t1 - t) - DP_H[k]) - z_h);
    }

    // split up y into y1 + y2 and compute (y1+y2)*(t1+t2)
    let y1 = with_lo_zero(y);
    let p_l = (y - y1) * t1 + y * t2;
    let mut p_h = y1 * t1;
    let mut z = p_l + p_h;
    let mut j = hi(z) as i32;
    let i = lo(z);
    if j >= 0x4090_0000 {
        // z >= 1024
        if ((j - 0x4090_0000) as u32 | i) != 0 {
            return s * POW_HUGE * POW_HUGE;
        }
        if p_l + OVT > z - p_h {
            return s * POW_HUGE * POW_HUGE;
        }
    } else if (j & 0x7fff_ffff) >= 0x4090_cc00 {
        // z <= -1075
        if ((j.wrapping_sub(0xc090_cc00u32 as i32)) as u32 | i) != 0 {
            return s * POW_TINY * POW_TINY;
        }
        if p_l <= z - p_h {
            return s * POW_TINY * POW_TINY;
        }
    }

    // compute 2**(p_h + p_l)
    let i = j & 0x7fff_ffff;
    let mut k = (i >> 20) - 0x3ff;
    let mut n = 0i32;
    if i > 0x3fe0_0000 {
        // if |z| > 0.5, set n = [z+0.5]
        n = j + (0x0010_0000 >> (k + 1));
        k = ((n & 0x7fff_ffff) >> 20) - 0x3ff; // new k for n
        let t = from_words((n & !(0x000f_ffff >> k)) as u32, 0);
        n = ((n & 0x000f_ffff) | 0x0010_0000) >> (20 - k);
        if j < 0 {
            n = -n;
        }
        p_h -= t;
    }
    let t = with_lo_zero(p_l + p_h);
    let u = t * LG2_H;
    let v = (p_l - (t - p_h)) * LG2C + t * LG2_L;
    z = u + v;
    let w = v - (z - u);
    let t = z * z;
    let t1 = z - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))));
    let r = (z * t1) / (t1 - 2.0) - (w + z * w);
    z = 1.0 - (r - z);
    j = hi(z) as i32;
    j += n << 20;
    if (j >> 20) <= 0 {
        z = scalbn(z, n); // subnormal output
    } else {
        z = with_hi(z, j as u32);
    }
    s * z
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identities that hold in exact arithmetic, so they hold for any correct
    /// implementation on any platform.
    ///
    /// Deliberately **not** a comparison against `std`. The host libm is the
    /// thing this module exists not to trust: it differs by an ulp at runtime,
    /// and under cross-compilation `std`'s `powf` can be constant-folded by the
    /// *building* machine's math instead of the target's, so "agrees with std"
    /// is not even a stable statement. The real oracle is `tests/golden.rs`.
    #[test]
    fn satisfies_the_identities_it_must() {
        let close = |a: f64, b: f64, what: &str| {
            let scale = a.abs().max(b.abs()).max(1.0);
            assert!((a - b).abs() <= 1e-14 * scale, "{what}: {a} vs {b}");
        };
        for i in 0..2000 {
            let x = (i as f64 - 1000.0) * 0.0137;
            close(sin(x) * sin(x) + cos(x) * cos(x), 1.0, "sin² + cos²");
            close(tan(x) * cos(x), sin(x), "tan · cos = sin");
            close(sin(-x), -sin(x), "sin is odd");
            close(cos(-x), cos(x), "cos is even");
            close(atan(-x), -atan(x), "atan is odd");
            if x.abs() < 0.7 {
                close(atan(tan(x)), x, "atan ∘ tan");
            }
            if x > 0.0 {
                close(pow(x, 2.0), x * x, "pow(x, 2)");
                close(pow(x, 0.5) * pow(x, 0.5), x, "sqrt via pow");
                close(log(pow(x, 3.0)), 3.0 * log(x), "log(x³) = 3 log x");
                close(pow(2.0, log(x) / std::f64::consts::LN_2), x, "exp ∘ log");
            }
        }
        // Values that must be exact, not merely close.
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(atan(0.0), 0.0);
        assert_eq!(log(1.0), 0.0);
    }

    /// The whole reason this module exists, stated as a test: the kernel is
    /// self-contained, so two different machines cannot disagree about it.
    #[test]
    fn known_values_are_pinned_to_exact_bit_patterns() {
        // Captured from the oracle the golden files were produced on. If a
        // platform ever fails this, the golden files would have failed too.
        for (x, bits) in [
            (1.0f64, 0x3FEAED548F090CEEu64),
            (0.5, 0x3FDEAEE8744B05F0),
            (2.0, 0x3FED18F6EAD1B446),
        ] {
            assert_eq!(sin(x).to_bits(), bits, "sin({x})");
        }
        assert_eq!(cos(1.0).to_bits(), 0x3FE14A280FB5068Cu64);
        assert_eq!(atan(1.0).to_bits(), 0x3FE921FB54442D18u64);
        assert_eq!(log(2.0).to_bits(), 0x3FE62E42FEFA39EFu64);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(tan(0.5).to_bits(), 0x3FE17B4F5BF3474Au64);
    }

    #[test]
    fn huge_argument_reduction_works() {
        // Exercises kernel_rem_pio2, which the medium path never reaches.
        for x in [1e22, 1.234e30, 6.2831853e8, 1e100] {
            assert!((sin(x) - x.sin()).abs() < 1e-9, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 1e-9, "cos({x})");
        }
    }

    #[test]
    fn pow_edge_cases() {
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(pow(0.0, 0.0), 1.0);
        assert_eq!(pow(-2.0, 3.0), -8.0);
        assert_eq!(pow(4.0, 0.5), 2.0);
        assert!(pow(-2.0, 0.5).is_nan());
        assert_eq!(pow(1.5, 1.0), 1.5);
    }
}
