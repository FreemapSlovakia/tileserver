use crate::bbox::BBox;

const EARTH_RADIUS: f64 = 6_378_137.0; // Equatorial radius of the Earth in meters (WGS 84)

const HALF_CIRCUMFERENCE: f64 = std::f64::consts::PI * EARTH_RADIUS;

pub fn tile_bounds_to_epsg3857(x: u32, y: u32, z: u32, tile_size: u32) -> BBox<f64> {
    let tile_size = f64::from(tile_size);

    let total_pixels = tile_size * f64::from(z).exp2();
    let pixel_size = (2.0 * HALF_CIRCUMFERENCE) / total_pixels;

    let min_x = (f64::from(x) * tile_size).mul_add(pixel_size, -HALF_CIRCUMFERENCE);
    let max_y = (f64::from(y) * tile_size).mul_add(-pixel_size, HALF_CIRCUMFERENCE);

    let max_x = tile_size.mul_add(pixel_size, min_x);
    let min_y = tile_size.mul_add(-pixel_size, max_y);

    BBox {
        min_x,
        min_y,
        max_x,
        max_y,
    }
}
