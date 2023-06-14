use anyhow::Result;
use axum::{
    extract::{Json, Query},
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use dotenv::dotenv;
use headless_chrome::{types::PrintToPdfOptions, Browser, LaunchOptions};
use http_req::{request::Method, request::Request, uri::Uri};
use pdfium_render::prelude::*;
use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::env;
use std::net::SocketAddr;
use std::{fmt, str::FromStr};
use url::Url;

#[tokio::main]
async fn main() {
    // let addr = SocketAddr::from(([10, 0, 0, 75], 5000));
    let addr = SocketAddr::from(([10, 0, 0, 15], 4000));
    let app = Router::new()
        .route("/", get(handler))
        .route("/api", post(handle_post));

    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Debug, Serialize, Deserialize)]
struct Data {
    url: String,
}

async fn handle_post(data: Json<Data>) -> impl IntoResponse {
    println!("Received data: {:?}", data.url);

    if let Err(_) = Url::from_str(&data.url) {
        return Response::builder()
            .status(StatusCode::OK)
            .body("parse target url failure".to_string())
            .unwrap();
    } else {
        match get_text_headless(&data.url).await {
            Ok(res) => match get_summary_private(res).await {
                None => {
                    return Response::builder()
                        .status(StatusCode::OK)
                        .body("failed to create summary".to_string())
                        .unwrap()
                }

                Some(summary) => {
                    return Response::builder()
                        .status(StatusCode::OK)
                        .body(summary)
                        .unwrap()
                }
            },
            Err(_) => {
                return Response::builder()
                    .status(StatusCode::OK)
                    .body("failed to get text from webpage".to_string())
                    .unwrap();
            }
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct MyResponse {
    text: String,
}

async fn handler(Query(params): Query<Params>) -> String {
    "not able to parse url".to_string()
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Params {
    #[serde(default, deserialize_with = "empty_string_as_none")]
    url: Option<String>,
}

/// Serde deserialization decorator to map empty Strings to None,
fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom).map(Some),
    }
}

async fn get_text_headless(url: &str) -> anyhow::Result<String> {
    // set the headless Chrome to open a webpage in portrait mode of certain width and height
    // here in an iPad resolution, is a way to pursuade webserver to send less non-essential
    // data, and make the virtual browser to show the central content, for websites
    // with responsive design, with less clutter
    let options = LaunchOptions {
        headless: true,
        window_size: Some((820, 1180)),
        ..Default::default()
    };

    let browser = Browser::new(options)?;

    let tab = browser.new_tab()?;

    tab.navigate_to(url)?;
    tab.wait_until_navigated();

    let pdf_options: Option<PrintToPdfOptions> = Some(PrintToPdfOptions {
        landscape: Some(false),
        display_header_footer: Some(false),
        print_background: Some(false),
        scale: Some(0.5),
        paper_width: Some(11.0),
        paper_height: Some(17.0),
        margin_top: Some(0.1),
        margin_bottom: Some(0.1),
        margin_left: Some(0.1),
        margin_right: Some(0.1),
        page_ranges: Some("1-2".to_string()),
        ignore_invalid_page_ranges: Some(true),
        prefer_css_page_size: Some(false),
        transfer_mode: None,
        ..Default::default()
    });

    let pdf_data = tab.print_to_pdf(pdf_options)?;

    let pdf_as_vec = pdf_data.to_vec();
    //code below uses dynamically linked libpdfium.dylib on a M1 Mac
    //it takes some efforts to bind libpdfium on different platforms
    //please visit https://github.com/ajrcarey/pdfium-render/tree/master
    //for more details
    let text = Pdfium::new(
        Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
            "/home/ubuntu/pdfium/lib/",
            // "/Users/jaykchen/Downloads/pdfium-mac-arm64/lib/libpdfium.dylib",
        ))
        .or_else(|_| Pdfium::bind_to_system_library())?,
    )
    .load_pdf_from_byte_vec(pdf_as_vec, Some(""))?
    .pages()
    .iter()
    .map(|page| page.text().unwrap().all())
    .collect::<Vec<String>>()
    .join(" ");

    Ok(text)
}

pub async fn custom_gpt(sys_prompt: &str, u_prompt: &str, m_token: u16) -> Option<String> {
    let system_prompt = serde_json::json!(
        {"role": "system", "content": sys_prompt}
    );
    let user_prompt = serde_json::json!(
        {"role": "user", "content": u_prompt}
    );

    match chat(vec![system_prompt, user_prompt], m_token).await {
        Ok((res, _count)) => Some(res),
        Err(_) => None,
    }
}

pub async fn chat(
    message_obj: Vec<Value>,
    m_token: u16,
) -> Result<(String, String), anyhow::Error> {
    dotenv().ok();
    let api_token = env::var("OPENAI_API_TOKEN")?;

    let params = serde_json::json!({
      "model": "gpt-3.5-turbo",
      "messages": message_obj,
      "temperature": 0.7,
      "top_p": 1,
      "n": 1,
      "stream": false,
      "max_tokens": m_token,
      "presence_penalty": 0,
      "frequency_penalty": 0,
      "stop": "\n"
    });

    let uri = "https://api.openai.com/v1/chat/completions";

    let uri = Uri::try_from(uri)?;
    let mut writer = Vec::new();
    let body = serde_json::to_vec(&params)?;

    let bearer_token = format!("Bearer {}", api_token);
    let _response = Request::new(&uri)
        .method(Method::POST)
        .header("Authorization", &bearer_token)
        .header("Content-Type", "application/json")
        .header("Content-Length", &body.len())
        .body(&body)
        .send(&mut writer)?;

    let res = serde_json::from_slice::<ChatResponse>(&writer)?;
    let finish_reason = res.choices[0].finish_reason.clone();
    Ok((res.choices[0].message.content.to_string(), finish_reason))
}

#[derive(Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub choices: Vec<Choice>,
}

#[derive(Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: String,
}

#[derive(Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

async fn get_summary_private(inp: String) -> Option<String> {
    let mut feed_texts = inp.split_ascii_whitespace().collect::<Vec<&str>>();
    feed_texts.truncate(3000);

    let news_body = feed_texts.join(" ");

    let sys_prompt = "You're a new reporter AI.";
    let user_prompt = &format!("Given the news body text: {news_body}, which may include some irrelevant information, identify the key arguments and the article's conclusion. From these important elements, construct a succinct summary that encapsulates its news value, disregarding any unnecessary details.");

    custom_gpt(sys_prompt, user_prompt, 512).await
}
