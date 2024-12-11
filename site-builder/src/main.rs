use std::{cmp::Ordering, collections::BTreeMap, env, path::{Path, PathBuf}, sync::Arc};

use anyhow::Result;
use clap::Parser;
use reqwest::{header::{ACCEPT, USER_AGENT}, Client};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tera::{Context, Tera};
use time::{Date, OffsetDateTime};
use tokio::{fs as async_fs, task};
use tokio::time::{sleep as tokio_sleep, Duration as TokioDuration};

const OWNER: &str = "bferris413";
const GH_ACCEPT_JSON: &str = "application/vnd.github+json";
const GH_TOKEN_VAR: &str = "GH_TOKEN";

const USER_REPOS: &str = "https://api.github.com/user/repos?per_page={per_page}&page={page}";
const REPO_COMMITS: &str = "https://api.github.com/repos/{owner}/{repo}/commits?per_page={per_page}&page={page}";
const MAX_GRAPH_HISTORY: usize = 300;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to template index.html
    #[arg(long)]
    template_file_path: PathBuf,

    /// Path to write completed index.html
    #[arg(long)]
    out_file_path: PathBuf,

    /// Read commits from a file instead of the network for e.g. template dry-runs or testing
    #[arg(long, default_value_t = false, requires("input_file"))]
    no_fetch: bool,

    /// The file to read commits from, if --no-fetch was provided
    #[arg(long, requires("no_fetch"))]
    input_file: Option<PathBuf>,

    /// Saves fetched commits to a local file
    #[arg(long, conflicts_with("no_fetch"))]
    save_commits: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let per_page = 100;

    let commits = if args.no_fetch {
        read_commits(args.input_file.unwrap()).await?
    } else {
        let token = env::var(GH_TOKEN_VAR)?;
        let repos = fetch_owner_repos(&token, per_page).await?;
        fetch_commits(token, per_page, &repos[..]).await
    };

    if let Some(path) = args.save_commits {
        let commits = serde_json::to_string_pretty(&commits)?;
        async_fs::write(path, commits).await?;
    }

    let max_history = usize::min(MAX_GRAPH_HISTORY, commits.len());
    let html = populate_template(&commits[..max_history], args.template_file_path).await?;
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
                eprintln!("error fetching commits for repo '{repo_name}': {e}");
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
        tokio_sleep(TokioDuration::from_millis(50)).await;
    }

    let mut all_commits = Vec::new();
    for task in fetch_tasks {
        let commits = task.await.unwrap();
        all_commits.extend(commits.into_iter());
    }

    all_commits.sort_by(order_by_date_rev);
    all_commits
}

async fn read_commits(file: PathBuf) -> Result<Vec<RepoCommit>> {
    let commits_string = async_fs::read_to_string(file).await?;
    let commits = serde_json::from_str(&commits_string)?;
    
    Ok(commits)
}

async fn populate_template(commits: &[RepoCommit], index_template_path: PathBuf) -> Result<String> {
    let tp2 = index_template_path.clone();

    let tera = task::spawn_blocking(move || {
        let mut tera = Tera::default();
        tera.add_template_file(tp2, None)?;
        Ok::<_, tera::Error>(tera)
    }).await??;

    let ui_commits: Vec<_> = commits.iter().map(|rc| UiCommit::from(rc)).collect();
    let activity_stats = get_activity_stats(commits);

    let mut context = Context::new();
    context.insert("ui_commits", &ui_commits);
    context.insert("activity_stats", &activity_stats);

    let template_name = index_template_path.to_str().unwrap();
    let html = tera.render(template_name, &context)?;

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

fn get_activity_stats(commits: &[RepoCommit]) -> ActivityStats {
    let mut date_commits = map_commits(commits);
    fill_date_gaps(&mut date_commits);

    let max = *date_commits.iter().max_by(|c1, c2| c1.1.cmp(c2.1)).unwrap().1;
    let coords: Vec<_> = date_commits
        .into_iter()
        .enumerate()
        .map(|(index, (_date, count))| Point { x: index as u32, y: count })
        .collect();
    let len = coords.len();

    ActivityStats { max, coords, len }
}

/// Fills gaps between dates, up to but not including today, with <missing week number> -> 0.
fn fill_date_gaps(date_commits: &mut BTreeMap<WeekNumber, u32>) {
    let mut missing_weeks = vec![];
    let mut dates = date_commits.keys();
    let (mut w1, mut w2) = (dates.next(), dates.next());

    if w1.is_none() {
        return;
    }
    
    while let Some(next_plotted_week) = w2 {
        let mut working_week = w1.unwrap().0 + 1;
        while working_week != next_plotted_week.0 {
            missing_weeks.push((working_week, 0));
            working_week += 1;
        }
        w1 = w2;
        w2 = dates.next();
    }

    let this_week = WeekNumber::from(OffsetDateTime::now_utc().date());
    let last_plotted_week = date_commits.last_key_value().unwrap().0;
    let mut maybe_missing_week = last_plotted_week.0 + 1;

    while maybe_missing_week < this_week.0 {
        missing_weeks.push((maybe_missing_week, 0));
        maybe_missing_week += 1;
    }

    date_commits.extend(missing_weeks
                            .into_iter()
                            .map(|(week, n)| (WeekNumber(week), n)));
}

/// Collects commits into <commit date> -> <commit count>.
fn map_commits(commits: &[RepoCommit]) -> BTreeMap<WeekNumber, u32> {
    let mut date_commits = BTreeMap::new();
        for rc in commits {
            let date = rc.commit.commit.author.date.date();
            let week = WeekNumber::from(date);
            date_commits.entry(week)
                .and_modify(|v| *v += 1)
                .or_insert_with(|| 1);
        }

    date_commits
}

/// Stats we want to reference in the UI.
#[derive(Debug, Serialize)]
struct ActivityStats {
    coords: Vec<Point>,
    max: u32,
    len: usize
}

/// Coordinate point, used for plotting activity graph.
#[derive(Debug, Serialize)]
struct Point {
    x: u32,
    y: u32,
}

/// Commit information we want to display in the UI.
#[derive(Serialize)]
struct UiCommit {
    author_date: String,
    repo_name: Arc<String>,
    commit_msg: Option<String>,
}
impl From<&RepoCommit> for UiCommit {
    fn from(rc: &RepoCommit) -> Self {
        let date = rc.commit.commit.author.date; 
        let formatted_date = format!("{}-{:02}-{:02} {:02}:{:02}:{:02}Z", date.year(), date.month() as u8, date.day(), date.hour(), date.minute(), date.second());
        UiCommit {
            repo_name: rc.repo_name.clone(),
            author_date: formatted_date,
            commit_msg: rc.commit.commit.message.to_owned(),
        }
    }
}

///
#[derive(Deserialize, Serialize)]
struct RepoCommit {
    repo_name: Arc<String>,
    commit: Commit,

}

/// Parts of a GitHub commit we're interested in (may be incomplete compared to what the GH API returns).
#[derive(Debug, Eq, Deserialize, PartialEq, Serialize)]
struct Commit {
    commit: CommitData,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct CommitData {
    author: GitHubUser,
    committer: GitHubUser,
    message: Option<String>
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
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

/// A week numbered according to `date.year() * 52 + date.sunday_based_week()`.
#[derive(Debug, Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
struct WeekNumber(i32);
impl From<Date> for WeekNumber {
    fn from(date: Date) -> Self {
        let year = date.year();
        let week = date.sunday_based_week() as i32;
        let year_with_week = year * 52 + week;

        WeekNumber(year_with_week)
    }
} 

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_gaps() {
        let mut date_commits = BTreeMap::from_iter([
            (WeekNumber(0), 1),
            (WeekNumber(1), 1),
            (WeekNumber(2), 1),
            (WeekNumber(7), 1),
            (WeekNumber(8), 1),
            (WeekNumber(10), 1),
            (WeekNumber(11), 1),
            (WeekNumber(14), 1),
        ]);

        fill_date_gaps(&mut date_commits);
        dbg!(date_commits);
    }
}
