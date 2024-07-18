use crate::service::types::AppState;
use actix_web::{error::ErrorInternalServerError, http::StatusCode, Error, web::Data};
use anyhow::anyhow;
use jsonschema::{error::ValidationErrorKind, ValidationError};
use log::info;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{
        de::{self, IntoDeserializer},
        Deserialize, Serialize,
    };
use serde_json::{json, Map, Value};
use std::{
    env::VarError,
    fmt::{self, Display},
    str::FromStr,
};
use std::collections::HashMap;
use superposition_types::{result, Condition};
use crate::service::types::Tenant;

const CONFIG_TAG_REGEX: &str = "^[a-zA-Z0-9_-]{1,64}$";

//WARN Do NOT use this fxn inside api requests, instead add the required
//env to AppState and get value from there. As this panics, it should
//only be used for envs needed during app start.
pub fn get_from_env_unsafe<F>(name: &str) -> Result<F, VarError>
where
    F: FromStr,
    <F as FromStr>::Err: std::fmt::Debug,
{
    std::env::var(name)
        .map(|val| val.parse().unwrap())
        .map_err(|e| {
            log::info!("{name} env not found with error: {e}");
            e
        })
}

pub fn get_from_env_or_default<F>(name: &str, default: F) -> F
where
    F: FromStr + Display,
    <F as FromStr>::Err: std::fmt::Debug,
{
    match std::env::var(name) {
        Ok(env) => env.parse().unwrap(),
        Err(err) => {
            info!(
                "{name} ENV failed to load due to {err}, using default value {default}"
            );
            default
        }
    }
}

pub trait ToActixErr<T> {
    fn map_err_to_internal_server<B>(
        self,
        log_prefix: &str,
        err_body: B,
    ) -> Result<T, Error>
    where
        B: fmt::Debug + fmt::Display + 'static;
}

impl<T, E> ToActixErr<T> for Result<T, E>
where
    E: fmt::Debug,
{
    fn map_err_to_internal_server<B>(
        self,
        log_prefix: &str,
        err_body: B,
    ) -> Result<T, Error>
    where
        B: fmt::Debug + fmt::Display + 'static,
    {
        self.map_err(|e| {
            log::info!("{log_prefix}, err: {e:?}");
            ErrorInternalServerError(err_body)
        })
    }
}

pub fn deserialize_stringified_list<'de, D, I>(
    deserializer: D,
) -> std::result::Result<Vec<I>, D::Error>
where
    D: de::Deserializer<'de>,
    I: de::DeserializeOwned,
{
    struct StringVecVisitor<I>(std::marker::PhantomData<I>);

    impl<'de, I> de::Visitor<'de> for StringVecVisitor<I>
    where
        I: de::DeserializeOwned,
    {
        type Value = Vec<I>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str(
                "a string containing comma separated values eg: CREATED,INPROGRESS",
            )
        }

        fn visit_str<E>(self, v: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            let mut query_vector = Vec::new();
            for param in v.split(',') {
                let p: I = I::deserialize(param.into_deserializer())?;
                query_vector.push(p);
            }
            Ok(query_vector)
        }
    }

    deserializer.deserialize_any(StringVecVisitor(std::marker::PhantomData::<I>))
}

pub fn get_pod_info() -> (String, String) {
    let hostname: String = get_from_env_unsafe("HOSTNAME").expect("HOSTNAME is not set");
    let tokens = hostname
        .split('-')
        .map(str::to_string)
        .collect::<Vec<String>>();
    let mut tokens = tokens.iter().rev();
    let (pod_id, _replica_set, deployment_id) = (
        tokens.next().unwrap().to_owned(),
        tokens.next().unwrap().to_owned(),
        tokens.next().unwrap().to_owned(),
    );
    (pod_id, deployment_id)
}

pub fn extract_dimensions(context: &Condition) -> result::Result<Map<String, Value>> {
    // Assuming max 2-level nesting in context json logic

    let conditions: Vec<Value> = match (*context).get("and") {
        Some(conditions_json) => conditions_json
            .as_array()
            .ok_or(result::AppError::BadArgument("Error extracting dimensions, failed parsing conditions as an array. Ensure the context provided obeys the rules of JSON logic".into()))?
            .clone(),
        None => vec![Value::Object(context.to_owned().into())],
    };

    let mut dimension_tuples = Vec::new();
    for condition in &conditions {
        let condition_obj =
            condition
                .as_object()
                .ok_or(result::AppError::BadArgument(
                    "Failed to parse condition as an object. Ensure the context provided obeys the rules of JSON logic".to_string()
                ))?;
        let operators = condition_obj.keys();

        for operator in operators {
            let operands = condition_obj[operator].as_array().ok_or(result::AppError::BadArgument(
                    "Failed to parse operands as an arrays. Ensure the context provided obeys the rules of JSON logic"
                            .into()
            ))?;

            let (variable_name, variable_value) = get_variable_name_and_value(operands)?;

            dimension_tuples.push((String::from(variable_name), variable_value.clone()));
        }
    }

    Ok(Map::from_iter(dimension_tuples))
}

pub fn get_variable_name_and_value(operands: &[Value]) -> result::Result<(&str, &Value)> {
    let (obj_pos, variable_obj) = operands
        .iter()
        .enumerate()
        .find(|(_, operand)| {
            operand.is_object() && operand.as_object().unwrap().get("var").is_some()
        })
        .ok_or(result::AppError::BadArgument(
            "Failed to get variable name from operands list. Ensure the context provided obeys the rules of JSON logic"
                .into()
        ))?;

    let variable_name = variable_obj
        .as_object().and_then(|obj| obj.get("var")).and_then(|value| value.as_str())
        .ok_or(result::AppError::BadArgument(
            "Failed to get variable name as string. Ensure the context provided obeys the rules of JSON logic"
                .into()
        ))?;

    let value_pos = (obj_pos + 1) % 2;
    let variable_value =
        operands
            .get(value_pos)
            .ok_or(result::AppError::BadArgument(
                "Failed to get variable value from operands list. Ensure the context provided obeys the rules of JSON logic"
                    .into()
            ))?;

    Ok((variable_name, variable_value))
}

pub fn validation_err_to_str(errors: Vec<ValidationError>) -> Vec<String> {
    errors.into_iter().map(|error| {
        match error.kind {
            ValidationErrorKind::AdditionalItems { limit } => {
                format!("input array contain more items than expected, limit is {limit}")
            }
            ValidationErrorKind::AdditionalProperties { unexpected } => {
                format!("unexpected properties `{}`", unexpected.join(", "))
            }
            ValidationErrorKind::AnyOf => {
                "not valid under any of the schemas listed in the 'anyOf' keyword".to_string()
            }
            ValidationErrorKind::BacktrackLimitExceeded { error: _ } => {
                "backtrack limit exceeded while matching regex".to_string()
            }
            ValidationErrorKind::Constant { expected_value } => {
                format!("value doesn't match expected constant `{expected_value}`")
            }
            ValidationErrorKind::Contains => {
                "array doesn't contain items conforming to the specified schema".to_string()
            }
            ValidationErrorKind::ContentEncoding { content_encoding } => {
                format!("value doesn't respect the defined contentEncoding `{content_encoding}`")
            }
            ValidationErrorKind::ContentMediaType { content_media_type } => {
                format!("value doesn't respect the defined contentMediaType `{content_media_type}`")
            }
            ValidationErrorKind::Enum { options } => {
                format!("value doesn't match any of specified options {}", options)
            }
            ValidationErrorKind::ExclusiveMaximum { limit } => {
                format!("value is too large, limit is {limit}")
            }
            ValidationErrorKind::ExclusiveMinimum { limit } => {
                format!("value is too small, limit is {limit}")
            }
            ValidationErrorKind::FalseSchema => {
                "everything is invalid for `false` schema".to_string()
            }
            ValidationErrorKind::FileNotFound { error: _ } => {
                "referenced file not found".to_string()
            }
            ValidationErrorKind::Format { format } => {
                format!("value doesn't match the specified format `{}`", format)
            }
            ValidationErrorKind::FromUtf8 { error: _ } => {
                "invalid UTF-8 data".to_string()
            }
            ValidationErrorKind::InvalidReference { reference } => {
                format!("`{}` is not a valid reference", reference)
            }
            ValidationErrorKind::InvalidURL { error } => {
                format!("invalid URL: {}", error)
            }
            ValidationErrorKind::JSONParse { error } => {
                format!("error parsing JSON: {}", error)
            }
            ValidationErrorKind::MaxItems { limit } => {
                format!("too many items in array, limit is {}", limit)
            }
            ValidationErrorKind::Maximum { limit } => {
                format!("value is too large, maximum is {}", limit)
            }
            ValidationErrorKind::MaxLength { limit } => {
                format!("string is too long, maximum length is {}", limit)
            }
            ValidationErrorKind::MaxProperties { limit } => {
                format!("too many properties in object, limit is {}", limit)
            }
            ValidationErrorKind::MinItems { limit } => {
                format!("not enough items in array, minimum is {}", limit)
            }
            ValidationErrorKind::Minimum { limit } => {
                format!("value is too small, minimum is {}", limit)
            }
            ValidationErrorKind::MinLength { limit } => {
                format!("string is too short, minimum length is {}", limit)
            }
            ValidationErrorKind::MinProperties { limit } => {
                format!("not enough properties in object, minimum is {}", limit)
            }
            ValidationErrorKind::MultipleOf { multiple_of } => {
                format!("value is not a multiple of {}", multiple_of)
            }
            ValidationErrorKind::Not { schema } => {
                format!("negated schema `{}` failed validation", schema)
            }
            ValidationErrorKind::OneOfMultipleValid => {
                "value is valid under more than one schema listed in the 'oneOf' keyword".to_string()
            }
            ValidationErrorKind::OneOfNotValid => {
                "value is not valid under any of the schemas listed in the 'oneOf' keyword".to_string()
            }
            ValidationErrorKind::Pattern { pattern } => {
                format!("value doesn't match the pattern `{}`", pattern)
            }
            ValidationErrorKind::PropertyNames { error } => {
                format!("object property names are invalid: {}", error)
            }
            ValidationErrorKind::Required { property } => {
                format!("required property `{}` is missing", property)
            }
            ValidationErrorKind::Resolver { url, error } => {
                format!("error resolving reference `{}`: {}", url, error)
            }
            ValidationErrorKind::Schema => {
                "resolved schema failed to compile".to_string()
            }
            ValidationErrorKind::Type { kind } => {
                format!("value doesn't match the required type(s) `{:?}`", kind)
            }
            ValidationErrorKind::UnevaluatedProperties { unexpected } => {
                format!("unevaluated properties `{}`", unexpected.join(", "))
            }
            ValidationErrorKind::UniqueItems => {
                "array contains non-unique elements".to_string()
            }
            ValidationErrorKind::UnknownReferenceScheme { scheme } => {
                format!("unknown reference scheme `{}`", scheme)
            }
            ValidationErrorKind::Utf8 { error } => {
                format!("invalid UTF-8 string: {}", error)
            }
        }
    }).collect()
}

use once_cell::sync::Lazy;
static HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(reqwest::Client::new);

pub fn construct_request_headers(entries: &[(&str, &str)]) -> Result<HeaderMap, String> {
    entries
        .iter()
        .map(|(name, value)| {
            let h_name = HeaderName::from_str(name);
            let h_value = HeaderValue::from_str(value);

            match (h_name, h_value) {
                (Ok(n), Ok(v)) => Some((n, v)),
                _ => None,
            }
        })
        .collect::<Option<Vec<(HeaderName, HeaderValue)>>>()
        .map(HeaderMap::from_iter)
        .ok_or(String::from("failed to parse headers"))
}

pub async fn request<'a, T, R>(
    url: String,
    method: reqwest::Method,
    body: Option<T>,
    headers: HeaderMap,
) -> Result<R, reqwest::Error>
where
    T: serde::Serialize,
    R: serde::de::DeserializeOwned,
{
    let mut request_builder = HTTP_CLIENT.request(method.clone(), url).headers(headers);
    request_builder = match (method, body) {
        (reqwest::Method::GET | reqwest::Method::DELETE, _) => request_builder,
        (_, Some(data)) => request_builder.json(&data),
        _ => request_builder,
    };

    let response = request_builder.send().await?;

    response.json::<R>().await
}
pub fn generate_snowflake_id(state: &Data<AppState>) -> result::Result<i64> {
    let mut snowflake_generator = state.snowflake_generator.lock().map_err(|e| {
        log::error!("snowflake_id generation failed {}", e);
        result::AppError::UnexpectedError(anyhow!("snowflake_id generation failed {}", e))
    })?;
    let id = snowflake_generator.real_time_generate();
    // explicitly dropping snowflake_generator so that lock is released and it can be acquired in bulk-operations handler
    drop(snowflake_generator);
    Ok(id)
}

pub fn parse_config_tags(
    config_tags: Option<String>,
) -> result::Result<Option<Vec<String>>> {
    let regex = Regex::new(CONFIG_TAG_REGEX).map_err(|err| {
        log::error!("regex match failed for tags {}", err);
        result::AppError::UnexpectedError(anyhow!("Something went wrong"))
    })?;
    match config_tags {
        None => Ok(None),
        Some(val) => {
            let tags = val
                .split(',')
                .map(|s| {
                    if !regex.is_match(s) {
                        Err(result::AppError::BadArgument(
                            "Invalid config_tags value".to_string(),
                        ))
                    } else {
                        Ok(s.to_owned())
                    }
                })
                .collect::<result::Result<Vec<String>>>()?;
            Ok(Some(tags))
        }
    }
}

//TODO: Refactor the code below.

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TypeValidationResponse {
    result: bool,
}

pub async fn validate_inputs(
    key: &str,
    value: &serde_json::Value,
    _schema: &serde_json::Map<String, Value>, //Added In case needed in future.
    tenan: Tenant,
) -> Result<bool, Error> {
    // Calling type validation API of validate type of the key.

    let nyhost: String =
        get_from_env_unsafe("TYPE_CHECK_HASHMAP").expect("TYPE_CHECK_HASHMAP is not set");
    log::info!("TYPE_CHECK_HASHMAP: {nyhost}");
    let hashmap: HashMap<String, String> = serde_json::from_str(&nyhost)
        .expect("Failed to deserialize JSON string to HashMap");

    log::info!("HashMap: {:?}", hashmap);
    let tenant = &tenan.0;
    let nyurl = if hashmap.contains_key(tenant) {
    let host_url = hashmap.get(tenant).unwrap();
        format!("{host_url}/internal/typeCheck") // TODO:Change endpoints to master path
    } else {
        "null".to_string()
    };

    if nyurl == "null".to_string() {
        return Ok(true);
    }

    let nyclient = reqwest::Client::new();
    let nypayload = json!({
        "key": key,
        "value": value
    });
    let token: String = get_from_env_unsafe("TYPE_CHECK_TOKEN")
        .expect("Authentication token not found for type validation API.");
    let ny_response_res = nyclient
        .post(&nyurl)
        .json(&nypayload)
        .header("Authorization", token)
        .send()
        .await
        .map_err(|e| e.to_string());

    let response = match ny_response_res {
        Ok(response) => response,
        Err(e) => {
            log::error!("Failed to call nammayatri backend: {e}");
            return Err(ErrorInternalServerError(
                json!({"message": "Failed to call nammayatri backend"}),
            )
            .into());
        }
    };

    let decoded_resp_res = match response.status() {
            StatusCode::OK => response.json::<TypeValidationResponse>().await.map_err(|e| e.to_string()),
            error_code => return Err(ErrorInternalServerError(json!({
                "message": format!("Failed to decode response from nammayatri backend, errorCode: {}", error_code),
                })).into()),
        };

    let decoded_resp = match decoded_resp_res {
        Ok(result) => result,
        Err(err) => {
            log::info!("Failed to decode result from nammayatri backend");
            return Err(ErrorInternalServerError(
                    json!({"message": &("Failed to decode result from nammayatri backend ".to_string() + &err)}),
                ).into());
        }
    };
    return Ok(decoded_resp.result);
    // End of type validation API call
}

