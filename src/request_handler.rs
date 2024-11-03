use crate::gdal_reader::read_rgba_from_gdal;
use crate::size::Size;
use crate::xyz::tile_bounds_to_epsg3857;
use gdal::Dataset;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    Method, Request, Response, StatusCode,
};
use image::{codecs::jpeg::JpegEncoder, ImageEncoder};
use std::{cell::RefCell, io::Cursor, sync::Arc};
use tokio::runtime::Runtime;

thread_local! {
    static THREAD_LOCAL_DATA: RefCell<Option<Dataset>> = const {RefCell::new(None)};
}

pub async fn handle_request(
    pool: Arc<Runtime>,
    req: Request<Incoming>,
    path: Arc<String>,
) -> Result<Response<BoxBody<Bytes, std::io::Error>>, hyper::http::Error> {
    if req.method() != Method::GET {
        return Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(
                Full::new("Method not allowed".into())
                    .map_err(|e| match e {})
                    .boxed(),
            );
    }

    let parts: Vec<_> = req
        .uri()
        .path()
        .split('/')
        .skip(1)
        .map(|a| a.parse::<u32>().ok())
        .collect();

    match (
        parts.get(0).copied().flatten(),
        parts.get(1).copied().flatten(),
        parts.get(2).copied().flatten(),
    ) {
        (Some(zoom), Some(x), Some(y)) if parts.len() == 3 => {
            let bbox = tile_bounds_to_epsg3857(x, y, zoom, 256);

            let result = pool
                .spawn_blocking(move || {
                    THREAD_LOCAL_DATA.with(|data| {
                        let mut data = data.borrow_mut();

                        let raster = {
                            let ds = data.get_or_insert_with(|| {
                                Dataset::open(path.to_string()).expect("error opening dataset")
                            });

                            read_rgba_from_gdal(
                                ds,
                                bbox,
                                Size {
                                    width: 256f64,
                                    height: 256f64,
                                },
                                true,
                            )?
                        };

                        // let encoder = webp::Encoder::from_rgba(&raster, 256, 256);

                        // let webp = encoder.encode_lossless();

                        // Vec::from(&*webp)

                        let mut img_data = Vec::<u8>::new();

                        let cursor = Cursor::new(&mut img_data);

                        JpegEncoder::new_with_quality(cursor, 95).write_image(
                            &raster,
                            256,
                            256,
                            image::ExtendedColorType::Rgb8,
                        )?;

                        anyhow::Ok(Bytes::from(img_data))
                    })
                })
                .await;

            let result = result
                .map_err(anyhow::Error::new)
                .and_then(|inner_result| inner_result);

            result.map_or_else(
                |_| {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(
                            Full::new("Internal Server Error".into())
                                .map_err(|e| match e {})
                                .boxed(),
                        )
                },
                |message| {
                    Response::builder()
                        .status(StatusCode::OK)
                        // .header("Content-Type", "image/webp")
                        .header("Content-Type", "image/jpeg")
                        .header("Access-Control-Allow-Origin", "*")
                        .body(Full::new(message).map_err(|e| match e {}).boxed())
                },
            )
        }
        _ => Response::builder().status(StatusCode::BAD_REQUEST).body(
            Full::new("Bad request".into())
                .map_err(|e| match e {})
                .boxed(),
        ),
    }
}
