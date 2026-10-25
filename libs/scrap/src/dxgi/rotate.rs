// Added for desklib by Matt Timmermans, 2026. GNU AGPL v3; see NOTICE.md.

//! 32-bit pixel rotation, matching the semantics of libyuv's `ARGBRotate`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    Rotate0,
    /// Clockwise.
    Rotate90,
    Rotate180,
    /// Clockwise, i.e. 90 degrees counter-clockwise.
    Rotate270,
}

/// Rotates a `width` x `height` image of 4-byte pixels from `src` into `dst`.
///
/// `width` and `height` describe the source; for 90/270 the destination is `height` pixels wide.
pub fn argb_rotate(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
    width: usize,
    height: usize,
    rotation: Rotation,
) {
    for y in 0..height {
        let src_row = &src[y * src_stride..][..width * 4];
        for (x, px) in src_row.chunks_exact(4).enumerate() {
            let (dx, dy) = match rotation {
                Rotation::Rotate0 => (x, y),
                Rotation::Rotate90 => (height - 1 - y, x),
                Rotation::Rotate180 => (width - 1 - x, height - 1 - y),
                Rotation::Rotate270 => (y, width - 1 - x),
            };
            dst[dy * dst_stride + dx * 4..][..4].copy_from_slice(px);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 3x2 image whose pixels are numbered 0..6 in row-major order.
    fn rotate(rotation: Rotation) -> Vec<u8> {
        let src: Vec<u8> = (0..6u8).flat_map(|i| [i; 4]).collect();
        let (dw, _) = match rotation {
            Rotation::Rotate90 | Rotation::Rotate270 => (2, 3),
            _ => (3, 2),
        };
        let mut dst = vec![0u8; 24];
        argb_rotate(&src, 12, &mut dst, dw * 4, 3, 2, rotation);
        dst.chunks_exact(4).map(|p| p[0]).collect()
    }

    #[test]
    fn rotations() {
        // 0 1 2
        // 3 4 5
        assert_eq!(rotate(Rotation::Rotate0), [0, 1, 2, 3, 4, 5]);
        assert_eq!(rotate(Rotation::Rotate90), [3, 0, 4, 1, 5, 2]);
        assert_eq!(rotate(Rotation::Rotate180), [5, 4, 3, 2, 1, 0]);
        assert_eq!(rotate(Rotation::Rotate270), [2, 5, 1, 4, 0, 3]);
    }
}
