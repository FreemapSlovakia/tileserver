use std::borrow::Cow;

use crate::bbox::BBox;
use gdal::{errors::GdalError, raster::ResampleAlg, Dataset};
use itertools::Itertools;
use thiserror::Error;

pub enum Background {
    Alpha,
    Rgb(u8, u8, u8),
}

pub struct BackgroundError();

impl TryFrom<Cow<'_, str>> for Background {
    type Error = BackgroundError;

    fn try_from(value: Cow<'_, str>) -> Result<Self, Self::Error> {
        if value.len() != 6 {
            return Err(BackgroundError());
        }

        value
            .chars()
            .chunks(2)
            .into_iter()
            .map(|chunk| chunk.collect::<String>())
            .map(|c| u8::from_str_radix(&c, 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(|_| BackgroundError())
            .map(|rgb| Self::Rgb(rgb[0], rgb[1], rgb[2]))
    }
}

#[derive(Error, Debug)]
pub enum ReadError {
    #[error("band count error")]
    BandCountError,
    #[error("gdal error")]
    GdalError(#[from] GdalError),
}

pub fn read_rgba_from_gdal(
    dataset: &Dataset,
    result_bbox: BBox<f64>,
    size: (usize, usize),
    background: Background,
) -> Result<(bool, Vec<u8>), ReadError> {
    let input_count = dataset.raster_count();

    if !matches!(input_count, 3..=4) {
        return Err(ReadError::BandCountError);
    }

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

    let band_size = size.0 * size.1;

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

    let ww = (size.0 as f64 * (adj_source_width as f64 / source_width as f64)) as usize;

    let hh = (size.1 as f64 * (adj_source_height as f64 / source_height as f64)) as usize;

    let window = (adj_window_x, adj_window_y);

    let window_size = (adj_source_width, adj_source_height);

    let desired_size = (ww, hh);

    let off_x = if window_x == adj_window_x {
        0
    } else if pixel_min_x <= 0 && pixel_max_x >= raster_width as isize {
        (0.0 as f64 - size.0 as f64 * window_x as f64 / source_width as f64) as usize
    } else {
        size.0 - ww
    };

    let off_y = if window_y == adj_window_y {
        0
    } else if pixel_min_y <= 0 && pixel_max_y >= raster_height as isize {
        (0.0 - size.1 as f64 * window_y as f64 / source_height as f64) as usize
    } else {
        size.1 - hh
    };

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
                window,
                window_size,
                desired_size,
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
            window,
            window_size,
            desired_size,
            &mut mask_data,
            Some(ResampleAlg::NearestNeighbour),
        )?;

        band.read_into_slice::<u8>(
            window,
            window_size,
            desired_size,
            &mut source_band,
            Some(ResampleAlg::NearestNeighbour),
        )?;

        for y in 0..size.0.min(hh) {
            for x in 0..size.1.min(ww) {
                let data_index = y * ww + x;

                if mask_data[data_index] != 0 {
                    let result_index =
                        ((y + off_y) * size.0 + (x + off_x)) * result_count + band_index;

                    result_data[result_index] = alpha_band.as_ref().map_or_else(
                        || source_band[data_index],
                        |alpha_band| {
                            let alpha = u16::from(alpha_band[data_index]);

                            ((u16::from(source_band[data_index]) * alpha
                                + u16::from(result_data[result_index]) * (255 - alpha))
                                / 255) as u8
                        },
                    );
                }
            }
        }
    }

    // premultiply
    if result_count == 4 {
        for i in (0..result_data.len()).step_by(3) {
            let alpha = f32::from(result_data[i + 3]) / 255.0;

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
