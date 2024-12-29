use crate::gdal_reader::{read_rgba_from_gdal, Background, ReadError};
use crate::xyz::tile_bounds_to_epsg3857;
use gdal::Dataset;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    Method, Request, Response, StatusCode,
};
use image::ImageError;
use image::{codecs::jpeg::JpegEncoder, ImageEncoder};
use std::path::Path;
use std::{cell::RefCell, io::Cursor, sync::Arc};
use thiserror::Error;
use tokio::runtime::Runtime;
use tokio::task::JoinError;

thread_local! {
    static THREAD_LOCAL_DATA: RefCell<Option<Dataset>> = const {RefCell::new(None)};
}

enum Ext {
    Jpeg,
    // Png,
    Webp,
}

impl TryFrom<&str> for Ext {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            _ => Err(format!("unsupported extension {value}")),
        }
    }
}

#[derive(Error, Debug)]
enum FooErr {
    #[error("join error")]
    JoinError(#[from] JoinError),
    #[error("not acceptable")]
    NotAcceptable,
    #[error("gdal read error: {0}")]
    GdalReadError(#[from] ReadError),
    #[error("image encoding error: {0}")]
    ImageEncodingError(#[from] ImageError),
}

pub async fn handle_request(
    pool: Arc<Runtime>,
    req: Request<Incoming>,
    raster_path: &'static Path,
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

    let path = req.uri().path();

    let parts: Vec<_> = path.splitn(2, '.').collect();

    let ext: Result<Option<Ext>, _> = parts.get(1).map(|&x| x.try_into()).transpose();

    let Ok(ext) = ext else {
        return Response::builder().status(StatusCode::NOT_FOUND).body(
            Full::new("Not found".into())
                .map_err(|e| match e {})
                .boxed(),
        );
    };

    let parts: Vec<_> = parts
        .get(0)
        .copied()
        .unwrap_or_default()
        .get(1..)
        .unwrap_or_default()
        .splitn(3, '/')
        .map(|a| a.parse::<u32>().ok())
        .collect();

    match (
        parts.get(0).copied().flatten(),
        parts.get(1).copied().flatten(),
        parts.get(2).copied().flatten(),
    ) {
        (Some(zoom), Some(x), Some(y)) if parts.len() == 3 => {
            let bbox = tile_bounds_to_epsg3857(x, y, zoom, 256);

            pool.spawn_blocking(move || {
                THREAD_LOCAL_DATA.with(|data| {
                    let mut data = data.borrow_mut();

                    let (has_alpha, raster) = {
                        let ds = data.get_or_insert_with(|| {
                            Dataset::open(raster_path).expect("error opening dataset")
                        });

                        read_rgba_from_gdal(
                            ds,
                            bbox,
                            (256, 256),
                            Background::Rgb(255, 0, 0), // TODO or alpha by query or image format alpha support
                        )?
                    };

                    match ext {
                        Some(Ext::Webp) => {
                            let encoder = if has_alpha {
                                webp::Encoder::from_rgba(&raster, 256, 256)
                            } else {
                                webp::Encoder::from_rgb(&raster, 256, 256)
                            };

                            let webp = encoder.encode_lossless(); // TODO configurable quality

                            Ok(Bytes::from(Vec::from(&*webp)))
                        }
                        Some(Ext::Jpeg) => {
                            let mut img_data = Vec::<u8>::new();

                            let cursor = Cursor::new(&mut img_data);

                            JpegEncoder::new_with_quality(cursor, 95).write_image(
                                &raster,
                                256,
                                256,
                                image::ExtendedColorType::Rgb8,
                            )?;

                            Ok(Bytes::from(img_data))
                        }
                        None => Err(FooErr::NotAcceptable),
                    }
                })
            })
            .await
            .map_err(FooErr::JoinError)
            .and_then(|inner_result| inner_result)
            .map_or_else(
                |e| match e {
                    FooErr::NotAcceptable => {
                        Response::builder().status(StatusCode::NOT_ACCEPTABLE).body(
                            Full::new("Not acceptable".into())
                                .map_err(|e| match e {})
                                .boxed(),
                        )
                    }
                    _ => {
                        eprintln!("Error: {e}");

                        Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(
                                Full::new("Internal Server Error".into())
                                    .map_err(|e| match e {})
                                    .boxed(),
                            )
                    }
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
        _ => Response::builder().status(StatusCode::NOT_FOUND).body(
            Full::new("Not found".into())
                .map_err(|e| match e {})
                .boxed(),
        ),
    }
}
