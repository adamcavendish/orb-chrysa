use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::OrbChrysaError;
use crate::store::blob::BlobStore;
use crate::store::metadata::TokenStore;

use super::AppState;

pub async fn v2_check<M: TokenStore, B: BlobStore>(
    State(state): State<Arc<AppState<M, B>>>,
    req: axum::http::Request<axum::body::Body>,
) -> Response {
    if let Some(auth) = &state.auth {
        if let Some(token) = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
        {
            return match auth.validate_token::<M>(token, &state.core.metadata).await {
                Ok(_) => (
                    StatusCode::OK,
                    [("Docker-Distribution-API-Version", "registry/2.0")],
                )
                    .into_response(),
                Err(e) => e.into_response(),
            };
        }

        return OrbChrysaError::auth_required(auth.token_endpoint_url(), "orb-chrysa", "")
            .into_response();
    }
    (
        StatusCode::OK,
        [("Docker-Distribution-API-Version", "registry/2.0")],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::test_state;
    use axum::body::Body;
    use axum::http::Request;

    use tower::ServiceExt;

    fn router() -> axum::Router {
        let state = test_state();
        axum::Router::new()
            .route(
                "/v2/",
                axum::routing::get(
                    v2_check::<
                        crate::store::metadata::InMemoryMetadataStore,
                        crate::store::blob::InMemoryBlobStore,
                    >,
                ),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn v2_check_returns_ok() {
        let response = router()
            .oneshot(Request::builder().uri("/v2/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn v2_check_returns_docker_api_version_header() {
        let response = router()
            .oneshot(Request::builder().uri("/v2/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .get("Docker-Distribution-API-Version")
                .map(|v| v.to_str().unwrap()),
            Some("registry/2.0")
        );
    }
}
