use crate::{bbox::BBox, size::Size};
use gdal::{raster::ResampleAlg, Dataset};

pub fn read_rgba_from_gdal(
    dataset: &Dataset,
    scale: f64,
    bbox: BBox<f64>,
    size: Size<f64>,
    with_alpha: bool,
) -> Vec<u8> {
    let [gt_x_off, gt_x_width, _, gt_y_off, _, gt_y_width] = dataset.geo_transform().unwrap();

    let BBox {
        min_x,
        min_y,
        max_x,
        max_y,
    } = bbox;

    // Convert geographic coordinates (min_x, min_y, max_x, max_y) to pixel coordinates
    let pixel_min_x = ((min_x - gt_x_off) / gt_x_width).round() as isize;
    let pixel_max_x = ((max_x - gt_x_off) / gt_x_width).round() as isize;
    let pixel_max_y = ((min_y - gt_y_off) / gt_y_width).round() as isize;
    let pixel_min_y = ((max_y - gt_y_off) / gt_y_width).round() as isize;

    let window_x = pixel_min_x;
    let window_y = pixel_min_y;
    let source_width = (pixel_max_x - pixel_min_x) as usize;
    let source_height = (pixel_max_y - pixel_min_y) as usize;

    let w_scaled = (size.width as f64 * scale) as usize;

    let h_scaled = (size.height as f64 * scale) as usize;

    let band_size = w_scaled * h_scaled;

    let band_count = if with_alpha { 4 } else { 3 };

    let mut rgba_data = vec![0u8; band_size * band_count];

    let (raster_width, raster_height) = dataset.raster_size();

    // Adjust the window to fit within the raster bounds
    let adj_window_x = window_x.max(0).min(raster_width as isize);
    let adj_window_y = window_y.max(0).min(raster_height as isize);

    let adj_source_width = ((window_x + source_width as isize).min(raster_width as isize)
        - adj_window_x)
        .max(0) as usize;

    let adj_source_height = ((window_y + source_height as isize).min(raster_height as isize)
        - adj_window_y)
        .max(0) as usize;

    let ww = (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize;
    let hh = (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize;

    let mut data = vec![0u8; hh * ww];
    // let mut mask_data = vec![0u8; hh * ww];

    for band_index in 0..band_count {
        let band = dataset.rasterband(band_index + 1).unwrap();

        // let mask_band = band.open_mask_band().unwrap();

        // mask_band
        //     .read_into_slice::<u8>(
        //         (adj_window_x, adj_window_y),
        //         (adj_source_width, adj_source_height),
        //         (
        //             (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize,
        //             (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize,
        //         ), // Resampled size
        //         &mut mask_data,
        //         Some(ResampleAlg::NearestNeighbour),
        //     )
        //     .unwrap();

        band.read_into_slice::<u8>(
            (adj_window_x, adj_window_y),
            (adj_source_width, adj_source_height),
            (
                (w_scaled as f64 * (adj_source_width as f64 / source_width as f64)) as usize,
                (h_scaled as f64 * (adj_source_height as f64 / source_height as f64)) as usize,
            ), // Resampled size
            &mut data,
            Some(ResampleAlg::NearestNeighbour),
        )
        .unwrap();

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

                let rgba_index = ((y + off_y) * w_scaled + (x + off_x)) * band_count + band_index;

                rgba_data[rgba_index] =
                //  if mask_data[data_index] != 0 {
                    data[data_index];
                // } else {
                //     255
                // };
            }
        }
    }

    // if with_alpha {
    //     for i in (0..rgba_data.len()).step_by(3) {
    //         let alpha = rgba_data[i + 3] as f32 / 255.0;

    //         let r = (rgba_data[i + 0] as f32 * alpha) as u8;
    //         let g = (rgba_data[i + 1] as f32 * alpha) as u8;
    //         let b = (rgba_data[i + 2] as f32 * alpha) as u8;

    //         rgba_data[i + 0] = r;
    //         rgba_data[i + 1] = g;
    //         rgba_data[i + 2] = b;
    //     }
    // }

    rgba_data
}
