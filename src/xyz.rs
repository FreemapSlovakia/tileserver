use crate::bbox::BBox;

const EARTH_RADIUS: f64 = 6_378_137.0; // Equatorial radius of the Earth in meters (WGS 84)

const HALF_CIRCUMFERENCE: f64 = std::f64::consts::PI * EARTH_RADIUS;

pub fn tile_bounds_to_epsg3857(x: u32, y: u32, z: u32, tile_size: u32) -> BBox<f64> {
    let total_pixels = tile_size as f64 * 2f64.powf(z as f64);
    let pixel_size = (2.0 * HALF_CIRCUMFERENCE) / total_pixels;

    let min_x = x as f64 * tile_size as f64 * pixel_size - HALF_CIRCUMFERENCE;
    let max_y = HALF_CIRCUMFERENCE - y as f64 * tile_size as f64 * pixel_size;

    let max_x = min_x + tile_size as f64 * pixel_size;
    let min_y = max_y - tile_size as f64 * pixel_size;

    BBox {
        min_x,
        min_y,
        max_x,
        max_y,
    }
}
