use crate::{bbox::BBox, size::Size};
use anyhow::{bail, Result};
use gdal::{raster::ResampleAlg, Dataset};

pub enum Background {
    Alpha,
    Rgb(u8, u8, u8),
}

pub fn read_rgba_from_gdal(
    dataset: &Dataset,
    result_bbox: BBox<f64>,
    size: Size<f64>,
    background: Background,
) -> Result<(bool, Vec<u8>)> {
    let [gt_x_off, gt_x_width, _, gt_y_off, _, gt_y_width] = dataset.geo_transform()?;

    let BBox {
        min_x,
        min_y,
        max_x,
        max_y,
    } = result_bbox;

    // Convert geographic coordinates (min_x, min_y, max_x, max_y) to pixel coordinates
    let pixel_min_x = ((min_x - gt_x_off) / gt_x_width).round() as isize;
    let pixel_max_x = ((max_x - gt_x_off) / gt_x_width).round() as isize;
    let pixel_max_y = ((min_y - gt_y_off) / gt_y_width).round() as isize;
    let pixel_min_y = ((max_y - gt_y_off) / gt_y_width).round() as isize;

    let window_x = pixel_min_x;
    let window_y = pixel_min_y;
    let source_width = pixel_max_x - pixel_min_x;
    let source_height = pixel_max_y - pixel_min_y;

    let w_scaled = size.width as usize;

    let h_scaled = size.height as usize;

    let band_size = w_scaled * h_scaled;

    let input_count = dataset.raster_count();

    if !matches!(input_count, 3..=4) {
        bail!("input is not rgb or rgba");
    }

    // TODO consider mask

    let result_count = if input_count == 4 && matches!(background, Background::Alpha) {
        4
    } else {
        3
    };

    let (raster_width, raster_height) = dataset.raster_size();

    // Adjust the window to fit within the raster bounds
    let adj_window_x = window_x.max(0).min(raster_width as isize);
    let adj_window_y = window_y.max(0).min(raster_height as isize);

    let adj_source_width: usize =
        ((window_x + source_width).min(raster_width as isize) - adj_window_x).max(0) as usize;

    let adj_source_height =
        ((window_y + source_height).min(raster_height as isize) - adj_window_y).max(0) as usize;

    let ww = (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize;
    let hh = (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize;

    let mut source_band = vec![0u8; hh * ww];

    let mut result_data = match (background, result_count) {
        (Background::Rgb(r, g, b), 4) => vec![r, g, b, 255]
            .into_iter()
            .cycle()
            .take(band_size * result_count)
            .collect::<Vec<u8>>(),

        (Background::Rgb(r, g, b), 3) => vec![r, g, b]
            .into_iter()
            .cycle()
            .take(band_size * result_count)
            .collect::<Vec<u8>>(),

        _ => vec![0u8; band_size * result_count],
    };

    let alpha_band = if result_count == 4 {
        Some({
            let mut source_band = vec![0u8; hh * ww];

            dataset.rasterband(4)?.read_into_slice::<u8>(
                (adj_window_x, adj_window_y),
                (adj_source_width, adj_source_height),
                (
                    (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize,
                    (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize,
                ),
                &mut source_band,
                Some(ResampleAlg::NearestNeighbour),
            )?;

            source_band
        })
    } else {
        None
    };

    for band_index in 0..result_count {
        let band = dataset.rasterband(band_index + 1)?;

        // band.mask_flags()?.

        let mask_band = band.open_mask_band()?;

        let mut mask_data = vec![0u8; hh * ww];

        mask_band.read_into_slice::<u8>(
            (adj_window_x, adj_window_y),
            (adj_source_width, adj_source_height),
            (
                (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize,
                (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize,
            ),
            &mut mask_data,
            Some(ResampleAlg::NearestNeighbour),
        )?;

        band.read_into_slice::<u8>(
            (adj_window_x, adj_window_y),
            (adj_source_width, adj_source_height),
            (
                (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize,
                (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize,
            ),
            &mut source_band,
            Some(ResampleAlg::NearestNeighbour),
        )?;

        for y in 0..w_scaled.min(hh) {
            for x in 0..h_scaled.min(ww) {
                let data_index = y * ww + x;

                let off_y = if window_y == adj_window_y {
                    0
                } else {
                    h_scaled - hh
                };

                let off_x = if window_x == adj_window_x {
                    0
                } else {
                    w_scaled - ww
                };

                if mask_data[data_index] != 0 {
                    let result_index =
                        ((y + off_y) * w_scaled + (x + off_x)) * result_count + band_index;

                    result_data[result_index] = alpha_band.as_ref().map_or_else(
                        || source_band[data_index],
                        |alpha_band| {
                            let alpha = u16::from(alpha_band[data_index]);

                            ((source_band[data_index] as u16 * alpha
                                + result_data[result_index] as u16 * (255 - alpha))
                                / 255) as u8
                        },
                    )
                }
            }
        }
    }

    // premultiply
    if result_count == 4 {
        for i in (0..result_data.len()).step_by(3) {
            let alpha = result_data[i + 3] as f32 / 255.0;

            let r = (f32::from(result_data[i + 0]) * alpha) as u8;
            let g = (f32::from(result_data[i + 1]) * alpha) as u8;
            let b = (f32::from(result_data[i + 2]) * alpha) as u8;

            result_data[i + 0] = r;
            result_data[i + 1] = g;
            result_data[i + 2] = b;
        }
    }

    Ok((result_count == 4, result_data))
}
