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
use std::convert::Infallible;
use std::path::Path;
use std::{cell::RefCell, io::Cursor, sync::Arc};
use tokio::runtime::Runtime;
use tokio::task::JoinError;
use url::Url;
use webp::WebPEncodingError;

thread_local! {
    static THREAD_LOCAL_DATA: RefCell<Option<Dataset>> = const {RefCell::new(None)};
}

enum ImageType {
    Jpeg,
    // Png,
    Webp,
}

impl TryFrom<&str> for ImageType {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            _ => Err(format!("unsupported extension {value}")),
        }
    }
}

#[derive(thiserror::Error, Debug)]
enum ProcessingError {
    #[error("join error")]
    JoinError(#[from] JoinError),

    #[error("not acceptable")]
    HttpError(StatusCode, Option<&'static str>),

    #[error("gdal read error: {0}")]
    GdalReadError(#[from] ReadError),

    #[error("image encoding error: {0}")]
    ImageEncodingError(#[from] ImageError),

    #[error("image encoding error")]
    WebPEncodingError(WebPEncodingError),
}

#[derive(thiserror::Error, Debug)]
pub enum BodyError {
    #[error("infallible")]
    Infillable(Infallible),
}

pub async fn handle_request(
    pool: Arc<Runtime>,
    req: Request<Incoming>,
    raster_path: &'static Path,
) -> Result<Response<BoxBody<Bytes, BodyError>>, hyper::http::Error> {
    if req.method() != Method::GET {
        return http_error(StatusCode::METHOD_NOT_ALLOWED);
    }

    let url = Url::parse(&req.uri().to_string()).unwrap();

    let path = url.path();

    let mut size: u32 = 256;

    let mut quality = 75.0;

    let mut background = Background::Alpha;

    for pair in url.query_pairs() {
        match pair.0.as_ref() {
            "background" | "bg" => {
                background = match pair.1.try_into() {
                    Ok(bg) => bg,
                    Err(_) => return http_error(StatusCode::BAD_REQUEST),
                }
            }
            "quality" | "q" => {
                quality = match pair.1.parse::<f32>() {
                    Ok(quality) => quality,
                    Err(_) => return http_error(StatusCode::BAD_REQUEST),
                }
            }
            "size" => {
                size = match pair.1.parse() {
                    Ok(size) => size,
                    Err(_) => return http_error(StatusCode::BAD_REQUEST),
                }
            }
            _ => {}
        }
    }

    let parts: Vec<_> = path.splitn(2, '.').collect();

    let ext: Result<Option<ImageType>, _> = parts.get(1).map(|&x| x.try_into()).transpose();

    let Ok(ext) = ext else {
        return http_error(StatusCode::NOT_FOUND);
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
            let bbox = tile_bounds_to_epsg3857(x, y, zoom, size);

            pool.spawn_blocking(move || {
                THREAD_LOCAL_DATA.with(|data| {
                    let mut data = data.borrow_mut();

                    let (has_alpha, raster) = {
                        let ds = data.get_or_insert_with(|| {
                            Dataset::open(raster_path).expect("error opening dataset")
                        });

                        read_rgba_from_gdal(ds, bbox, (size as usize, size as usize), background)?
                    };

                    match ext {
                        Some(ImageType::Webp) => {
                            let encoder = if has_alpha {
                                webp::Encoder::from_rgba(&raster, size, size)
                            } else {
                                webp::Encoder::from_rgb(&raster, size, size)
                            };

                            let webp = encoder
                                .encode_simple(quality == 100.0, quality)
                                .map_err(ProcessingError::WebPEncodingError)?;

                            Ok((ImageType::Webp, Bytes::from(Vec::from(&*webp))))
                        }
                        Some(ImageType::Jpeg) => {
                            let mut img_data = Vec::<u8>::new();

                            let cursor = Cursor::new(&mut img_data);

                            JpegEncoder::new_with_quality(cursor, (quality * 2.55).round() as u8)
                                .write_image(&raster, size, size, image::ExtendedColorType::Rgb8)?;

                            Ok((ImageType::Jpeg, Bytes::from(img_data)))
                        }
                        None => Err(ProcessingError::HttpError(StatusCode::NOT_FOUND, None)),
                    }
                })
            })
            .await
            .map_err(ProcessingError::JoinError)
            .and_then(|inner_result| inner_result)
            .map_or_else(
                |e| {
                    if let ProcessingError::HttpError(sc, message) = e {
                        http_error_msg(sc, message.unwrap_or_else(|| sc.as_str()))
                    } else {
                        eprintln!("Error: {e}");

                        http_error(StatusCode::INTERNAL_SERVER_ERROR)
                    }
                },
                |message| {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(
                            "Content-Type",
                            match message.0 {
                                ImageType::Jpeg => "image/jpeg",
                                ImageType::Webp => "image/webp",
                            },
                        )
                        .header("Access-Control-Allow-Origin", "*")
                        .body(Full::new(message.1).map_err(|e| match e {}).boxed())
                },
            )
        }
        _ => http_error(StatusCode::NOT_FOUND),
    }
}

fn http_error(sc: StatusCode) -> Result<Response<BoxBody<Bytes, BodyError>>, hyper::http::Error> {
    http_error_msg(sc, sc.as_str())
}

fn http_error_msg(
    sc: StatusCode,
    message: &str,
) -> Result<Response<BoxBody<Bytes, BodyError>>, hyper::http::Error> {
    Response::builder().status(sc).body(
        Full::new(Bytes::from(message.to_owned()))
            .map_err(BodyError::Infillable)
            .boxed(),
    )
}
