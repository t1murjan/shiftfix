//! Значок программы, нарисованный кодом: скруглённый квадрат с градиентом и
//! двумя стрелками Shift. Нужен трею (ARGB) и окну GUI (RGBA); тот же рисунок
//! лежит в assets/shiftfix.svg.

const MARGIN: f32 = 0.04;
const RADIUS: f32 = 0.22;
const ARROWS: [(f32, f32); 2] = [(0.375, 1.0), (0.625, 0.85)]; // (центр, непрозрачность)
const APEX_Y: f32 = 0.25;
const HEAD_Y: f32 = 0.53;
const HEAD_HALF: f32 = 0.125;
const STEM_HALF: f32 = 0.055;
const STEM_Y: f32 = 0.75;

const ACTIVE: [[f32; 3]; 2] = [[61.0, 174.0, 233.0], [124.0, 92.0, 224.0]]; // #3daee9 → #7c5ce0
const PAUSED: [[f32; 3]; 2] = [[154.0, 163.0, 173.0], [95.0, 103.0, 114.0]]; // #9aa3ad → #5f6772

fn in_rounded_square(x: f32, y: f32) -> bool {
    let (lo, hi) = (MARGIN + RADIUS, 1.0 - MARGIN - RADIUS);
    let dx = (lo - x).max(x - hi).max(0.0);
    let dy = (lo - y).max(y - hi).max(0.0);
    let inside = MARGIN..=1.0 - MARGIN;
    inside.contains(&x) && inside.contains(&y) && dx * dx + dy * dy <= RADIUS * RADIUS
}

fn in_arrow(x: f32, y: f32, cx: f32) -> bool {
    let in_stem = (cx - STEM_HALF..=cx + STEM_HALF).contains(&x) && (HEAD_Y..=STEM_Y).contains(&y);
    let in_head = (APEX_Y..=HEAD_Y).contains(&y)
        && (x - cx).abs() <= HEAD_HALF * (y - APEX_Y) / (HEAD_Y - APEX_Y);
    in_stem || in_head
}

/// Пиксели RGBA (без премультипликации), `size × size`.
pub fn rgba(size: u32, active: bool) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let [from, to] = if active { ACTIVE } else { PAUSED };
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut cover, mut white) = (0.0f32, 0.0f32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    if in_rounded_square(x, y) {
                        cover += 1.0;
                        white += ARROWS
                            .iter()
                            .find(|(cx, _)| in_arrow(x, y, *cx))
                            .map_or(0.0, |(_, a)| *a);
                    }
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            let alpha = cover / n;
            let t = (px + py) as f32 / (2 * size) as f32;
            let mut pixel = [0u8; 4];
            for c in 0..3 {
                let base = from[c] + (to[c] - from[c]) * t;
                let mix = if cover > 0.0 {
                    (base * (cover - white) + 255.0 * white) / cover
                } else {
                    0.0
                };
                pixel[c] = mix.round().clamp(0.0, 255.0) as u8;
            }
            pixel[3] = (alpha * 255.0).round() as u8;
            out.extend_from_slice(&pixel);
        }
    }
    out
}

/// ARGB32 в сетевом порядке байт — формат StatusNotifierItem.
pub(crate) fn argb(size: u32, active: bool) -> Vec<u8> {
    let pixels = rgba(size, active);
    let (pixels, _) = pixels.as_chunks::<4>();
    pixels
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_transparent_corners_and_white_arrows() {
        let size = 64;
        let pixels = rgba(size, true);
        let at = |x: u32, y: u32| {
            &pixels[((y * size + x) * 4) as usize..((y * size + x) * 4 + 4) as usize]
        };
        assert_eq!(at(0, 0)[3], 0); // угол прозрачный
        assert_eq!(at(24, 40), [255, 255, 255, 255]); // ножка левой стрелки
        let bg = at(32, 56);
        assert!(bg[2] > bg[0] && bg[3] == 255, "{bg:?}"); // градиент, синий канал главный
        assert_eq!(argb(2, false).len(), 16);
    }
}
