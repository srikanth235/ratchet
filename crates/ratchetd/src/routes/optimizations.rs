//! Optimizations and what hangs off them: candidates, ratchet history, spend, report, pull request, receipt.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ratchet_core::ports::CostReconcileReport;
use ratchet_core::{Calibration, EffectiveProtected};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::AppState;
use super::extract::{ApiJson, ApiJsonOrDefault, ApiQuery, ApiResult};
use crate::api::*;
use crate::service::ServiceError;

pub fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_optimizations, create_optimization))
        .routes(routes!(get_optimization))
        .routes(routes!(start_optimization))
        .routes(routes!(stop_optimization))
        .routes(routes!(ratchet_history))
        .routes(routes!(winning_change))
        .routes(routes!(candidates))
        .routes(routes!(spend))
        .routes(routes!(reconcile_costs))
        .routes(routes!(calibration))
        .routes(routes!(protected_paths))
        .routes(routes!(receipt))
        .routes(routes!(open_pull_request))
        .routes(routes!(report))
}

/// Everything the Optimizations screen shows: KPI cards, the featured run's chart, the table, the activity feed.
#[utoipa::path(
    get,
    path = "/v1/optimizations",
    tag = "optimizations",
    operation_id = "listOptimizations",
    params(RangeQuery),
    responses((status = 200, body = OptimizationsSnapshot))
)]
async fn list_optimizations(
    State(s): State<AppState>,
    ApiQuery(q): ApiQuery<RangeQuery>,
) -> ApiResult<Json<OptimizationsSnapshot>> {
    Ok(Json(
        s.service
            .list_optimizations(q.range.unwrap_or_default())
            .await?,
    ))
}

/// Create an optimization (a draft; `start` runs it). With `demo` it runs in simulation with no setup.
#[utoipa::path(
    post,
    path = "/v1/optimizations",
    tag = "optimizations",
    operation_id = "createOptimization",
    request_body = CreateOptimizationInput,
    responses((status = 201, body = CreatedOptimization))
)]
async fn create_optimization(
    State(s): State<AppState>,
    ApiJson(input): ApiJson<CreateOptimizationInput>,
) -> ApiResult<(StatusCode, Json<CreatedOptimization>)> {
    Ok((
        StatusCode::CREATED,
        Json(s.service.create_optimization(input).await?),
    ))
}

/// One optimization with its agents, bench pipeline, baseline vs best, receipt and calibration.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}",
    tag = "optimizations",
    operation_id = "getOptimization",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = OptimizationDetail))
)]
async fn get_optimization(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<OptimizationDetail>> {
    Ok(Json(s.service.get_optimization(&id).await?))
}

/// Start the baseline and the agents. Idempotent while running; refused (409) once the optimization ended.
#[utoipa::path(
    post,
    path = "/v1/optimizations/{id}/start",
    tag = "optimizations",
    operation_id = "startOptimization",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 204, description = "Started"), (status = 409, body = ApiError, description = "Already ended or not ready"))
)]
async fn start_optimization(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    s.service.start_optimization(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The kill switch: stop the agents and end the run.
#[utoipa::path(
    post,
    path = "/v1/optimizations/{id}/stop",
    tag = "optimizations",
    operation_id = "stopOptimization",
    params(("id" = String, Path, description = "Optimization id")),
    request_body(content = StopOptimizationInput, description = "Optional; an empty body is fine."),
    responses((status = 204, description = "Stopped"))
)]
async fn stop_optimization(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiJsonOrDefault(input): ApiJsonOrDefault<StopOptimizationInput>,
) -> ApiResult<StatusCode> {
    if input
        .reason
        .as_deref()
        .is_some_and(|r| r.chars().count() > 500)
    {
        return Err(ServiceError::Invalid(
            "reason is at most 500 characters".into(),
        ));
    }
    s.service.stop_optimization(&id, input.reason).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The ratchet chart: baseline step, one step per verified win, a point per candidate.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/ratchet",
    tag = "optimizations",
    operation_id = "getRatchetHistory",
    params(("id" = String, Path, description = "Optimization id"), MetricQuery),
    responses((status = 200, body = RatchetSeries))
)]
async fn ratchet_history(
    State(s): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(q): ApiQuery<MetricQuery>,
) -> ApiResult<Json<RatchetSeries>> {
    Ok(Json(s.service.ratchet_history(&id, q.metric).await?))
}

/// The squashed result: best verified commit vs the base ref, as the pull request will show it.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/winning-change",
    tag = "optimizations",
    operation_id = "getWinningChange",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = WinningChangeVm))
)]
async fn winning_change(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<WinningChangeVm>> {
    Ok(Json(s.service.winning_change(&id).await?))
}

/// Every candidate commit with its verdict and bench results.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/candidates",
    tag = "optimizations",
    operation_id = "listCandidates",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = Vec<CandidateVm>))
)]
async fn candidates(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<CandidateVm>>> {
    Ok(Json(s.service.candidates(&id).await?))
}

/// Spend by category and by agent, estimated vs reconciled, and the cumulative series.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/spend",
    tag = "optimizations",
    operation_id = "getSpend",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = SpendVm))
)]
async fn spend(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<SpendVm>> {
    Ok(Json(s.service.spend(&id).await?))
}

/// Compare the ledger's estimates with billing and record correction rows. Idempotent; `pending` while billing lags.
#[utoipa::path(
    post,
    path = "/v1/optimizations/{id}/spend/reconciliations",
    tag = "optimizations",
    operation_id = "reconcileSpend",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = CostReconcileReport))
)]
async fn reconcile_costs(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<CostReconcileReport>> {
    Ok(Json(s.service.reconcile_costs(&id).await?))
}

/// The auto-calibrated statistics (repeats, the gain a win must beat). 404 until the baseline has run.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/calibration",
    tag = "optimizations",
    operation_id = "getCalibration",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = Calibration))
)]
async fn calibration(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Calibration>> {
    s.service
        .calibration(&id)
        .await?
        .map(Json)
        .ok_or_else(|| ServiceError::NotFound(format!("optimization {id} has no baseline yet")))
}

/// Every protected path with why: the automatic set plus the user's list.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/protected-paths",
    tag = "optimizations",
    operation_id = "getProtectedPaths",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = EffectiveProtected))
)]
async fn protected_paths(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EffectiveProtected>> {
    Ok(Json(s.service.protected_paths(&id).await?))
}

/// The receipt of the sandbox teardown that covered this run. 404 while the environment is still alive.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/receipt",
    tag = "environments",
    operation_id = "getReceipt",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = TeardownReceiptVm))
)]
async fn receipt(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TeardownReceiptVm>> {
    s.service.receipt(&id).await?.map(Json).ok_or_else(|| {
        ServiceError::NotFound(format!("no teardown receipt for optimization {id} yet"))
    })
}

/// Open the pull request with the squashed verified wins. 409 without a verified win.
#[utoipa::path(
    post,
    path = "/v1/optimizations/{id}/pull-request",
    tag = "reports",
    operation_id = "openPullRequest",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 201, body = PullRequestVm), (status = 409, body = ApiError, description = "No verified win yet"))
)]
async fn open_pull_request(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<(StatusCode, Json<PullRequestVm>)> {
    Ok((
        StatusCode::CREATED,
        Json(s.service.open_pull_request(&id).await?),
    ))
}

/// The markdown report of the optimization.
#[utoipa::path(
    get,
    path = "/v1/optimizations/{id}/report",
    tag = "reports",
    operation_id = "getReport",
    params(("id" = String, Path, description = "Optimization id")),
    responses((status = 200, body = ReportVm))
)]
async fn report(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<ReportVm>> {
    Ok(Json(s.service.report(&id).await?))
}
