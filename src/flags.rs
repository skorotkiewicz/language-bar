use ksni::Icon;

pub fn icon(layout: &str) -> Icon {
    const W: usize = 32;
    const H: usize = 32;
    let white = [255, 255, 255];
    let red = [220, 35, 50];
    let blue = [0, 55, 140];
    let name = layout.to_lowercase();
    let mut data = vec![0; W * H * 4];
    for y in 5..27 {
        for x in 0..W {
            let row = y - 5;
            let color = if name.starts_with("polish") {
                if row < 11 { white } else { red }
            } else if name.starts_with("german") {
                [[20, 20, 20], red, [255, 205, 0]][(row * 3 / 22).min(2)]
            } else if name.starts_with("english (uk") {
                let diagonal = ((x * 21 / 31) as i32 - row as i32).abs() < 2
                    || ((31 - x) * 21 / 31) as i32 == row as i32;
                if (13..19).contains(&x) || (9..13).contains(&row) {
                    red
                } else if (11..21).contains(&x) || (7..15).contains(&row) || diagonal {
                    white
                } else {
                    blue
                }
            } else if name.starts_with("english") {
                if x < 14 && row < 12 {
                    // At tray size, single pixels stand in for the US stars.
                    if x % 3 == 1 && row % 2 == 1 {
                        white
                    } else {
                        blue
                    }
                } else if (row * 13 / 22) % 2 == 0 {
                    red
                } else {
                    white
                }
            } else if name.starts_with("french") {
                [blue, white, red][(x * 3 / W).min(2)]
            } else if name.starts_with("italian") {
                [[0, 145, 70], white, red][(x * 3 / W).min(2)]
            } else if name.starts_with("russian") {
                [white, blue, red][(row * 3 / 22).min(2)]
            } else if name.starts_with("ukrainian") {
                if row < 11 { blue } else { [255, 210, 0] }
            } else {
                // ponytail: unlisted layouts use a keyboard icon; add a flag mapping when needed.
                let key = (4..28).contains(&x)
                    && (5..17).contains(&row)
                    && (x % 5 < 3 && row % 4 < 2 || row >= 14 && (9..23).contains(&x));
                if key { white } else { [65, 65, 65] }
            };
            let offset = (y * W + x) * 4;
            data[offset..offset + 4].copy_from_slice(&[255, color[0], color[1], color[2]]);
        }
    }
    Icon {
        width: W as i32,
        height: H as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_argb_with_transparent_padding() {
        let pl = icon("Polish");
        assert_eq!(pl.data.len(), 32 * 32 * 4);
        assert_eq!(&pl.data[..4], &[0, 0, 0, 0]);
        assert_eq!(&pl.data[5 * 32 * 4..5 * 32 * 4 + 4], &[255, 255, 255, 255]);
        assert_eq!(&pl.data[26 * 32 * 4..26 * 32 * 4 + 4], &[255, 220, 35, 50]);
        assert_ne!(icon("English (US)").data, pl.data);
        assert_ne!(icon("German").data, pl.data);
        assert_ne!(icon("English (UK)").data, icon("English (US)").data);
        assert_ne!(icon("Unknown").data, pl.data);
    }
}
