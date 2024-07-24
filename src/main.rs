mod bbox;
mod gdal_reader;
mod size;
mod xyz;

use gdal::Dataset;
use gdal_reader::read_rgba_from_gdal;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{server::conn::http1, service::service_fn, Method, Response};
use hyper::{Error, Request, StatusCode};
use hyper_util::rt::TokioIo;
use image::codecs::jpeg::JpegEncoder;
use image::ImageEncoder;
use size::Size;
use std::cell::RefMut;
use std::io::Cursor;
use std::{cell::RefCell, net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use xyz::tile_bounds_to_epsg3857;

thread_local! {
    static THREAD_LOCAL_DATA: RefCell<Option<Dataset>> = RefCell::new(None);
}

async fn handle_request(
    pool: Arc<Runtime>,
    req: Request<Incoming>,
) -> Result<Response<BoxBody<Bytes, std::io::Error>>, Error> {
    if req.method() != Method::GET {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(
                Full::new("Method not allowed".into())
                    .map_err(|e| match e {})
                    .boxed(),
            )
            .unwrap());
    }

    let parts: Vec<_> = req
        .uri()
        .path()
        .split('/')
        .skip(1)
        .map(|a| a.parse::<u32>().ok())
        .collect();

    println!("{:?}", parts);

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

                        if data.is_none() {
                            *data = Some(
                                // Dataset::open("/home/martin/OSM/build/final.tif")
                                Dataset::open("/media/martin/14TB/ofmozaika/sk-wrapped.tif")
                                    .expect("error opening dataset"),
                            );
                        }

                        let ds = RefMut::map(data, |opt| opt.as_mut().unwrap());

                        let raster = read_rgba_from_gdal(
                            &ds,
                            1f64,
                            bbox,
                            Size {
                                width: 256f64,
                                height: 256f64,
                            },
                            false
                        );

                        let mut img_data = Vec::<u8>::new();

                        let cursor = Cursor::new(&mut img_data);

                        JpegEncoder::new(cursor)
                            .write_image(&raster, 256, 256, image::ExtendedColorType::Rgb8)
                            .unwrap();

                        Bytes::from(img_data)
                    })
                })
                .await;

            match result {
                Ok(message) => Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "image/jpeg")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(Full::new(message.into()).map_err(|e| match e {}).boxed())
                    .unwrap()),
                Err(_) => Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(
                        Full::new("Internal Server Error".into())
                            .map_err(|e| match e {})
                            .boxed(),
                    )
                    .unwrap()),
            }
        }
        _ => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(
                    Full::new("Bad request".into())
                        .map_err(|e| match e {})
                        .boxed(),
                )
                .unwrap())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Create a dedicated Tokio runtime for Dataset tasks.
    let dataset_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4) // Set the number of worker threads for the dedicated runtime.
            .max_blocking_threads(24) // Set the maximum number of blocking threads.
            .enable_all()
            .build()
            .unwrap(),
    );

    let addr = SocketAddr::from(([127, 0, 0, 1], 3003));
    let listener = TcpListener::bind(addr).await?;

    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let pool = dataset_runtime.clone();

        let sfn = service_fn(move |req| {
            let pool = pool.clone();
            async move { handle_request(pool, req).await }
        });

        tokio::spawn(async move {
            if let Err(err) = http1::Builder::new().serve_connection(io, sfn).await {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}
