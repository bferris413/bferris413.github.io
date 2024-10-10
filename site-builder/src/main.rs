use std::{cmp::Ordering, env, path::{Path, PathBuf}, sync::Arc};

use anyhow::Result;
use clap::Parser;
use reqwest::{header::{ACCEPT, USER_AGENT}, Client};
use serde::{de::DeserializeOwned, Deserialize};
use time::OffsetDateTime;
use tokio::fs as async_fs;
use tokio::time::{sleep as tokio_sleep, Duration};

const OWNER: &str = "bferris413";
const GH_ACCEPT_JSON: &str = "application/vnd.github+json";
const GH_TOKEN_VAR: &str = "GH_TOKEN";

const USER_REPOS: &str = "https://api.github.com/user/repos?per_page={per_page}&page={page}";
const REPO_COMMITS: &str = "https://api.github.com/repos/{owner}/{repo}/commits?per_page={per_page}&page={page}";

// debug -------------------------------------
const REPLACE_TEXT: &str = "{replace}";
// -------------------------------------------

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to template index.html
    #[arg(long)]
    template_file_path: PathBuf,

    /// Path to write completed index.html
    #[arg(long)]
    out_file_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let token = env::var(GH_TOKEN_VAR)?;
    let per_page = 100;
    dbg!(&args.template_file_path);
    dbg!(&args.out_file_path);

    let repos = fetch_owner_repos(&token, per_page).await?;
    let commits = fetch_commits(token, per_page, &repos[..]).await;
    let html = populate_template(&commits[..100], &args.template_file_path).await?;

    write_output(&html, &args.out_file_path).await
}

async fn fetch_owner_repos(token: &str, per_page: usize) -> Result<Vec<Repo>> {
    let repos = authd_get_all::<Repo>(USER_REPOS, &token, per_page).await?
        .into_iter()
        .filter(|r| r.owner.login == OWNER)
        .collect();

    Ok(repos)
}

/// Fetches all commits for each repo.
/// 
/// The returned vector is sorted in descending order.
async fn fetch_commits(token: String, per_page: usize, repos: &[Repo]) -> Vec<RepoCommit> {
    let repo_commits = REPO_COMMITS.replace("{owner}", OWNER);
    let repo_commits_url = Arc::new(repo_commits);
    let token = Arc::new(token);

    // spawn a task per repo to fetch commits for that repo
    let mut fetch_tasks = Vec::with_capacity(repos.len());
    for repo in repos.iter() {
        let repo_name = Arc::new(repo.name.clone());
        let repo_commits_url = repo_commits_url.clone();
        let token = token.clone();

        let fetch_task = tokio::spawn(async move {
            let commits_url = repo_commits_url.replace("{repo}", &repo_name);

            let commit_resp = authd_get_all::<Commit>(&commits_url, &*token, per_page).await;
            if let Err(e) = commit_resp {
                eprintln!("error fetching {commits_url}: {e}");
                return Vec::new();
            };

            let repo_commits: Vec<RepoCommit> = commit_resp.unwrap()
                .into_iter()
                .map(|commit| RepoCommit { commit, repo_name: repo_name.clone() })
                .collect();

            repo_commits
        });

        fetch_tasks.push(fetch_task);
    }

    while fetch_tasks.iter().any(|t| ! t.is_finished()) {
        tokio_sleep(Duration::from_millis(50)).await;
    }

    let mut all_commits = Vec::new();
    for task in fetch_tasks {
        let commits = task.await.unwrap();
        all_commits.extend(commits.into_iter());
    }

    all_commits.sort_by(order_by_date_rev);
    all_commits
}

async fn populate_template(commits: &[RepoCommit], index_template_path: &Path) -> Result<String> {
    let template = async_fs::read_to_string(index_template_path).await?;
    let mut replacement_text = String::new();
    for commit in commits.iter() {
        let date = commit.commit.commit.author.date;
        let repo = &commit.repo_name;
        let message = commit.commit.commit.message.as_deref().unwrap_or("");
        replacement_text.push_str(&format!("{date:35}{repo:25}{message}\n"));
    }
    let html = template.replace(REPLACE_TEXT, &replacement_text);

    Ok(html)
}

async fn write_output(html: &str, out_file_path: &Path) -> Result<()> {
    async_fs::write(out_file_path, html).await?;
    Ok(())
}

async fn authd_get_all<T: DeserializeOwned>(
    template_url: &str,
    token: &str,
    per_page: usize,
) -> Result<Vec<T>> {
    let per_page_url = template_url.replace("{per_page}", &per_page.to_string());
    let mut page = 1;
    let mut ts = Vec::new();

    loop {
        let url = per_page_url.replace("{page}", &page.to_string());
        let response_ts: Vec<T> = authd_get(&url, token).await?;

        let should_break = response_ts.len() < per_page;
        ts.extend(response_ts.into_iter());

        if should_break {
            break;
        }
        page += 1;
    }

    Ok(ts)
}

async fn authd_get<T: DeserializeOwned>(url: &str, token: &str) -> Result<Vec<T>> {
    let client = Client::new();
    let response = client.get(url)
        .header(USER_AGENT, OWNER)
        .header(ACCEPT, GH_ACCEPT_JSON)
        .bearer_auth(token)
        .send()
        .await?;

    let t: Vec<T> = response.json().await?;
    Ok(t)
}

fn order_by_date_rev(first: &RepoCommit, second: &RepoCommit) -> Ordering {
    second.commit.commit.author.date.cmp(&first.commit.commit.author.date)
}

struct RepoCommit {
    repo_name: Arc<String>,
    commit: Commit,

}

#[derive(Debug, Eq, Deserialize, PartialEq)]
struct Commit {
    commit: CommitData,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct CommitData {
    author: GitHubUser,
    committer: GitHubUser,
    message: Option<String>
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct GitHubUser {
    name: String,
    email: String,
    #[serde(with = "time::serde::iso8601")]
    date: OffsetDateTime,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct Repo {
    name: String,
    owner: Owner,
    private: bool,
    html_url: String,
    description: Option<String>,
    topics: Vec<String>,
    #[serde(with = "time::serde::iso8601")]
    updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct Owner {
    login: String,
}

