use anyhow::{bail, Context, Result};
use base64::Engine;
use clap::Parser;
use reqwest::blocking::Client;
use reqwest::blocking::RequestBuilder;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const PWD: &'static str = "Vansblue30@@";

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Create Bamboo deployment versions and deploy them via REST"
)]
struct Args {
    #[arg(long, env = "BAMBOO_URL", default_value = "https://bamboo.bri.co.id")]
    bamboo_url: String,

    #[arg(long, env = "BAMBOO_USERNAME", default_value = "90186782")]
    username: Option<String>,

    #[arg(long, env = "BAMBOO_PASSWORD", default_value = PWD)]
    password: Option<String>,

    #[arg(
        long,
        env = "BAMBOO_TOKEN",
        help = "Token used by bamboo-mcp-server; supports 'user:token' basic token or bearer token"
    )]
    token: Option<String>,

    #[arg(long)]
    deployment_project_id: u64,

    #[arg(long)]
    environment_id: u64,

    #[arg(long)]
    plan_result_key: String,

    #[arg(
        long,
        help = "Override generated name; default depends on environment convention"
    )]
    name: Option<String>,

    #[arg(long, default_value = "Created by bamboo-release helper")]
    description: String,

    #[arg(long)]
    dry_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateVersionRequest<'a> {
    name: &'a str,
    plan_result_key: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BuildResult {
    plan_name: Option<String>,
    build_result_key: Option<String>,
    plan: Option<BuildPlan>,
}

#[derive(Deserialize)]
struct BuildPlan {
    #[serde(rename = "shortName")]
    short_name: Option<String>,
}

#[derive(Deserialize)]
struct VersionList {
    versions: Option<Vec<DeploymentVersion>>,
}

#[derive(Deserialize)]
struct DeploymentVersion {
    id: Option<u64>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct DeploymentProject {
    environments: Vec<DeploymentEnvironment>,
}

#[derive(Deserialize)]
struct DeploymentEnvironment {
    id: u64,
    name: String,
}

enum EnvironmentKind {
    Dev,
    Prerelease,
    Preploy,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let bamboo_url = args.bamboo_url.trim_end_matches('/');

    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("failed to create HTTP client")?;

    let build_url = format!(
        "{}/rest/api/latest/result/{}?expand=plan",
        bamboo_url, args.plan_result_key
    );
    let versions_url = format!(
        "{}/rest/api/latest/deploy/project/{}/versions?max-results=1000",
        bamboo_url, args.deployment_project_id
    );
    let deployment_project_url = format!(
        "{}/rest/api/latest/deploy/project/{}",
        bamboo_url, args.deployment_project_id
    );

    let branch_name = if args.dry_run && args.name.is_some() {
        "<skipped-by-name-override>".to_string()
    } else {
        let build: BuildResult = get_json(&client, &args, &build_url)
            .with_context(|| format!("failed to read build result {}", args.plan_result_key))?;
        build_branch_name(&build)?
    };

    let name = match args.name.clone() {
        Some(name) => name,
        None => {
            let versions = list_versions(&client, &args, &versions_url)
                .context("failed to read deployment versions")?;
            let environment_name =
                deployment_environment_name(&client, &args, &deployment_project_url)
                    .context("failed to read deployment environment")?;
            next_release_name(
                &branch_name,
                &environment_name,
                &args.plan_result_key,
                &versions,
            )?
        }
    };

    let create_url = format!(
        "{}/rest/api/latest/deploy/project/{}/version",
        bamboo_url, args.deployment_project_id
    );

    let payload = CreateVersionRequest {
        name: &name,
        plan_result_key: &args.plan_result_key,
    };

    let deploy_url = format!(
        "{}/rest/api/latest/queue/deployment?environmentId={}&versionId=<created-version-id>",
        bamboo_url, args.environment_id
    );

    if args.dry_run {
        println!("branchName: {}", branch_name);
        println!("releaseName: {}", name);
        println!("POST {}", create_url);
        println!("{}", serde_json::to_string_pretty(&payload)?);
        println!("POST {}", deploy_url);
        return Ok(());
    }

    let version = create_version(&client, &args, &create_url, &payload)?;
    let version_id = version
        .id
        .context("Bamboo create-version response did not include id")?;
    let version_name = version.name.as_deref().unwrap_or(&name);
    println!(
        "created deployment version: id={} name={}",
        version_id, version_name
    );

    let deployment = trigger_deployment(&client, &args, bamboo_url, version_id)?;
    println!("{}", serde_json::to_string_pretty(&deployment)?);

    Ok(())
}

fn get_json<T: for<'de> Deserialize<'de>>(client: &Client, args: &Args, url: &str) -> Result<T> {
    let response = authenticate(client.get(url), args)?
        .header(ACCEPT, "application/json")
        .send()
        .with_context(|| format!("failed to send GET {}", url))?;
    parse_response(response)
}

fn post_json<T: for<'de> Deserialize<'de>, B: Serialize>(
    client: &Client,
    args: &Args,
    url: &str,
    body: &B,
) -> Result<T> {
    let response = authenticate(client.post(url), args)?
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .json(body)
        .send()
        .with_context(|| format!("failed to send POST {}", url))?;
    parse_response(response)
}

fn parse_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
) -> Result<T> {
    let status = response.status();
    let body = response.text().context("failed to read Bamboo response")?;
    if !status.is_success() {
        bail!("Bamboo returned {}: {}", status, body);
    }
    serde_json::from_str(&body).context("Bamboo returned non-JSON response")
}

fn build_branch_name(build: &BuildResult) -> Result<String> {
    if let Some(short_name) = build
        .plan
        .as_ref()
        .and_then(|plan| plan.short_name.as_ref())
    {
        return Ok(short_name.clone());
    }
    if let Some(plan_name) = &build.plan_name {
        return Ok(plan_name.clone());
    }
    bail!(
        "could not infer branch name from build result {}",
        build.build_result_key.as_deref().unwrap_or("<unknown>")
    )
}

fn list_versions(client: &Client, args: &Args, url: &str) -> Result<Vec<DeploymentVersion>> {
    let value: Value = get_json(client, args, url)?;

    if let Ok(list) = serde_json::from_value::<VersionList>(value.clone()) {
        if let Some(versions) = list.versions {
            return Ok(versions);
        }
    }

    if let Some(items) = value
        .get("versions")
        .and_then(|versions| versions.get("version"))
    {
        return serde_json::from_value(items.clone()).context("failed to parse versions.version");
    }
    if let Some(items) = value.get("version") {
        return serde_json::from_value(items.clone()).context("failed to parse version list");
    }
    if value.is_array() {
        return serde_json::from_value(value).context("failed to parse version array");
    }

    bail!("could not find deployment versions in Bamboo response")
}

fn deployment_environment_name(client: &Client, args: &Args, url: &str) -> Result<String> {
    let project: DeploymentProject = get_json(client, args, url)?;
    project
        .environments
        .into_iter()
        .find(|environment| environment.id == args.environment_id)
        .map(|environment| environment.name)
        .with_context(|| format!("environment id {} not found", args.environment_id))
}

fn next_release_name(
    branch_name: &str,
    environment_name: &str,
    plan_result_key: &str,
    versions: &[DeploymentVersion],
) -> Result<String> {
    let prefix = release_prefix(branch_name, environment_name)?;
    let mut next = build_number(plan_result_key)?;
    let used_numbers: Vec<u64> = versions
        .iter()
        .filter_map(|version| version.name.as_deref())
        .filter_map(|name| name.strip_prefix(&prefix))
        .filter_map(|suffix| suffix.parse::<u64>().ok())
        .collect();

    while used_numbers.contains(&next) {
        next += 1;
    }

    Ok(format!("{}{}", prefix, next))
}

fn release_prefix(branch_name: &str, environment_name: &str) -> Result<String> {
    match environment_kind(environment_name)? {
        EnvironmentKind::Dev => Ok("release-".to_string()),
        EnvironmentKind::Prerelease => Ok("prerelease-".to_string()),
        EnvironmentKind::Preploy => Ok(format!("{}-", branch_name)),
    }
}

fn build_number(plan_result_key: &str) -> Result<u64> {
    plan_result_key
        .rsplit_once('-')
        .and_then(|(_, number)| number.parse::<u64>().ok())
        .with_context(|| format!("could not parse build number from {}", plan_result_key))
}

fn environment_kind(environment_name: &str) -> Result<EnvironmentKind> {
    let normalized = environment_name.to_ascii_lowercase();
    if normalized.contains("preploy") {
        return Ok(EnvironmentKind::Preploy);
    }
    if normalized.contains("prestage")
        || normalized.contains("prestaging")
        || normalized.contains("prerelease")
    {
        return Ok(EnvironmentKind::Prerelease);
    }
    if normalized == "dev" || normalized.contains("development") {
        return Ok(EnvironmentKind::Dev);
    }
    bail!(
        "unsupported environment name '{}'; use --name to override release naming",
        environment_name
    )
}

fn create_version(
    client: &Client,
    args: &Args,
    url: &str,
    payload: &CreateVersionRequest,
) -> Result<DeploymentVersion> {
    post_json(client, args, url, payload).context("failed to create deployment version")
}

fn trigger_deployment(
    client: &Client,
    args: &Args,
    bamboo_url: &str,
    version_id: u64,
) -> Result<Value> {
    let url = format!(
        "{}/rest/api/latest/queue/deployment?environmentId={}&versionId={}",
        bamboo_url, args.environment_id, version_id
    );
    let response = client.post(&url);
    let response = authenticate(response, args)?
        .header(ACCEPT, "application/json")
        .send()
        .with_context(|| format!("failed to send POST {}", url))?;
    parse_response(response).context("failed to trigger deployment")
}

fn authenticate(request: RequestBuilder, args: &Args) -> Result<RequestBuilder> {
    if let (Some(username), Some(password)) = (&args.username, &args.password) {
        return Ok(request.basic_auth(username, Some(password)));
    }

    if let Some(token) = &args.token {
        if token.contains(':') {
            let encoded = base64::engine::general_purpose::STANDARD.encode(token);
            return Ok(request.header(AUTHORIZATION, format!("Basic {}", encoded)));
        }
        return Ok(request.bearer_auth(token));
    }

    bail!("set BAMBOO_TOKEN or BAMBOO_USERNAME/BAMBOO_PASSWORD")
}
