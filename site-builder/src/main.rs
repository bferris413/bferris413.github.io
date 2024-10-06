use std::{env, sync::Arc};

use anyhow::Result;
use reqwest::{header::{ACCEPT, USER_AGENT}, Client};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;
use time::{OffsetDateTime, PrimitiveDateTime};

const OWNER: &str = "bferris413";
const GH_ACCEPT_JSON: &str = "application/vnd.github+json";
const GH_TOKEN_VAR: &str = "GH_TOKEN";

const USER_REPOS: &str = "https://api.github.com/user/repos?per_page={per_page}&page={page}";
const REPO_COMMITS: &str = "https://api.github.com/repos/{owner}/{repo}/commits?per_page={per_page}&page={page}";

#[tokio::main]
async fn main() -> Result<()> {
    let token = env::var(GH_TOKEN_VAR)?;
    let per_page = 100;

    let repos: Vec<Repo> = authd_get_all::<Repo>(USER_REPOS, &token, per_page).await?
        .into_iter()
        .filter(|r| r.owner.login == OWNER)
        .collect();

    let repo_commits = REPO_COMMITS.replace("{owner}", OWNER);
    let mut commits = Vec::new();
    for repo in repos.iter() {
        let repo_name = Arc::new(repo.name.clone());
        let commits_url = repo_commits.replace("{repo}", &repo.name);

        let commit_resp = authd_get_all::<Commit>(&commits_url, &token, per_page).await;
        if let Err(e) = commit_resp {
            eprintln!("fetch err: {e}");
            continue;
        };

        let repo_commits: Vec<RepoCommit> = commit_resp.unwrap()
            .into_iter()
            .map(|commit| RepoCommit { commit, repo_name: repo_name.clone() })
            .collect();

        commits.extend(repo_commits.into_iter());
    }

    commits.sort_by(|first, second| first.commit.commit.author.date.cmp(&second.commit.commit.author.date));

    for commit in commits.iter() {
        println!("{:35}{:25}{:?}", commit.commit.commit.author.date, commit.repo_name, commit.commit.commit.message);
    }
    
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
        println!("fetching {url}");
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

