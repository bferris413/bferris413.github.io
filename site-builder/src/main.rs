use std::{
    cmp::Ordering,
    collections::BTreeMap,
    env,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail, Result};
use clap::{
    builder::{PathBufValueParser, TypedValueParser},
    Parser,
};
use markdown::{
    mdast::{Heading, Node as MarkdownNode, Text},
    ParseOptions,
};
use reqwest::{
    header::{ACCEPT, USER_AGENT},
    Client,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tera::{Context, Tera};
use time::{Date, OffsetDateTime};
use tokio::time::{sleep as tokio_sleep, Duration as TokioDuration};
use tokio::{fs as async_fs, task};

const OWNER: &str = "bferris413";
const GH_ACCEPT_JSON: &str = "application/vnd.github+json";
const GH_TOKEN_VAR: &str = "GH_TOKEN";
const IP_API_TOKEN_VAR: &str = "IP_API_TOKEN";

const USER_REPOS: &str = "https://api.github.com/user/repos?per_page={per_page}&page={page}";
const REPO_COMMITS: &str =
    "https://api.github.com/repos/{owner}/{repo}/commits?per_page={per_page}&page={page}";
const IP_GEO: &str = "https://api.ipgeolocation.io/ipgeo?apiKey={api_key}";
const MAX_GRAPH_HISTORY: usize = 300;
const PER_PAGE: usize = 100;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Path to project directory
    #[arg(long, value_parser=PathBufValueParser::new().try_map(|p| p.canonicalize()))]
    project_dir: PathBuf,

    /// Path to write the populated project.
    ///
    /// If the directory exists, it must be empty or contain only web files. If it doesn't exist, it will be created.
    #[arg(long)]
    // #[arg(long, value_parser=PathBufValueParser::new().try_map(|p| p.canonicalize()))]
    out_dir: PathBuf,

    /// Exit after checking directory/file structure, without running anything else.
    #[arg(long, default_value_t = false)]
    sanity_check: bool,

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
    sanity_check(&args).await?;

    if args.sanity_check {
        return Ok(());
    }

    let posts_dir = args.project_dir.join("posts");
    let (posts, posts_hash) = read_md_posts(&posts_dir).await?;

    let (geo_data, commits, max_history) = fetch_commit_info(&args).await?;
    if let Some(path) = args.save_commits {
        let commits = serde_json::to_string_pretty(&commits)?;
        async_fs::write(path, commits).await?;
    }

    let template_dir = args.project_dir.join("templates");
    let tera = initialize_tera(&template_dir).await?;
    let site_content =
        populate_templates(&tera, &commits[..max_history], geo_data, &posts, posts_hash).await?;

    write_output(&site_content, &args.out_dir).await
}

async fn initialize_tera(templates_dir: &Path) -> Result<Tera> {
    let templates_glob = format!("{}/**/*.html", templates_dir.display());
    let tera = task::spawn_blocking(move || Tera::new(&templates_glob)).await??;
    Ok(tera)
}

/// Checks the project directory structure for the expected layout.
async fn sanity_check(args: &Cli) -> Result<()> {
    // check project directory
    eprintln!(
        "Checking directory structure at {}",
        args.project_dir.display()
    );

    if !args.project_dir.exists() {
        bail!("Project directory does not exist");
    }
    if !args.project_dir.is_dir() {
        bail!("Project directory is not a directory");
    }

    // check templates directory
    let templates_dir = args.project_dir.join("templates");
    if !templates_dir.exists() {
        bail!(
            "Templates directory {} does not exist",
            templates_dir.display()
        );
    }
    if !templates_dir.is_dir() {
        bail!(
            "Templates directory {} is not a directory",
            templates_dir.display()
        );
    }

    let index_template = templates_dir.join("index.html");
    if !index_template.exists() {
        bail!(
            "Index template file {} does not exist",
            index_template.display()
        );
    }
    if !index_template.is_file() {
        bail!(
            "Index template file {} is not a file",
            index_template.display()
        );
    }

    let posts_template = templates_dir.join("posts.html");
    if !posts_template.exists() {
        bail!(
            "Posts template file {} does not exist",
            posts_template.display()
        );
    }
    if !posts_template.is_file() {
        bail!(
            "Posts template file {} is not a file",
            posts_template.display()
        );
    }

    let individual_post_template = templates_dir.join("post.html");
    if !individual_post_template.exists() {
        bail!(
            "Individual post template file {} does not exist",
            individual_post_template.display()
        );
    }
    if !individual_post_template.is_file() {
        bail!(
            "Individual post template file {} is not a file",
            individual_post_template.display()
        );
    }

    // Posts directory checks
    let posts_dir = args.project_dir.join("posts");
    if !posts_dir.exists() {
        bail!("Posts directory {} does not exist", posts_dir.display());
    }
    if !posts_dir.is_dir() {
        bail!("Posts directory {} is not a directory", posts_dir.display());
    }

    // Static directory checks
    let static_dir = args.project_dir.join("static");
    if !static_dir.exists() {
        bail!("Statics directory {} does not exist", static_dir.display());
    }
    if !static_dir.is_dir() {
        bail!(
            "Statics directory {} is not a directory",
            static_dir.display()
        );
    }

    // Output directory checks
    if args.out_dir.exists() {
        if !args.out_dir.is_dir() {
            bail!(
                "Output directory {} is not a directory",
                args.out_dir.display()
            );
        }

        if !only_contains_web_stuff(&args.out_dir).await? {
            bail!(
                "Output directory {} contains non-web files (can't confidently auto-delete)",
                args.out_dir.display()
            );
        }
    }

    println!("Directory structures look good");
    Ok(())
}

async fn only_contains_web_stuff(out_dir: &Path) -> Result<bool> {
    let mut entries = async_fs::read_dir(out_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            // recursion with async requires indirection
            let web_check = Box::pin(only_contains_web_stuff(&path));
            if !web_check.await? {
                return Ok(false);
            }
        } else if !path
            .extension()
            .map_or(false, |ext| ext == "html" || ext == "css" || ext == "js")
        {
            return Ok(false);
        }
    }

    Ok(true)
}

async fn fetch_commit_info(args: &Cli) -> Result<(Option<GeoData>, Vec<RepoCommit>, usize)> {
    if args.no_fetch {
        let commits = read_commits(args.input_file.as_ref().unwrap()).await?;
        let max_history = usize::min(MAX_GRAPH_HISTORY, commits.len());
        Ok((None, commits, max_history))
    } else {
        let token = env::var(GH_TOKEN_VAR)?;
        let ip_api_token = env::var(IP_API_TOKEN_VAR)?;

        let repos = fetch_owner_repos(&token, PER_PAGE).await?;
        let geo_data = fetch_geo_data(&ip_api_token).await?;
        let commits = fetch_commits(token, PER_PAGE, &repos[..]).await;
        let max_history = usize::min(MAX_GRAPH_HISTORY, commits.len());
        Ok((Some(geo_data), commits, max_history))
    }
}

async fn read_md_posts(posts_dir: &Path) -> Result<(Vec<Post>, u64)> {
    let mut posts = Vec::new();
    let mut dir_entries = async_fs::read_dir(posts_dir).await?;

    while let Some(entry) = dir_entries.next_entry().await? {
        if entry.file_type().await?.is_file() {
            let file_path = entry.path();
            let filename = file_path.file_stem().unwrap().to_string_lossy().to_string();
            if filename == "posts" {
                // index, not a post
                continue;
            }
            let md_content = async_fs::read_to_string(&file_path).await?;
            let post = Post::try_new(md_content, filename)?;
            posts.push(post);
        }
    }

    posts.sort_by(|p1, p2| p1.post_number.cmp(&p2.post_number));

    let posts_hash = {
        let mut hasher = DefaultHasher::new();
        posts.hash(&mut hasher);
        hasher.finish()
    };

    Ok((posts, posts_hash))
}

async fn fetch_geo_data(token: &str) -> Result<GeoData> {
    let api = IP_GEO.replace("{api_key}", token);
    let client = Client::new();
    let response = client.get(api).send().await?;

    let geo_data: GeoData = response.json().await?;
    Ok(geo_data)
}

async fn fetch_owner_repos(token: &str, per_page: usize) -> Result<Vec<Repo>> {
    let repos = authd_get_all::<Repo>(USER_REPOS, &token, per_page)
        .await?
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

            let repo_commits: Vec<RepoCommit> = commit_resp
                .unwrap()
                .into_iter()
                .map(|commit| RepoCommit {
                    commit,
                    repo_name: repo_name.clone(),
                })
                .collect();

            repo_commits
        });

        fetch_tasks.push(fetch_task);
    }

    while fetch_tasks.iter().any(|t| !t.is_finished()) {
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

async fn read_commits(file: &Path) -> Result<Vec<RepoCommit>> {
    let commits_string = async_fs::read_to_string(file).await?;
    let commits = serde_json::from_str(&commits_string)?;

    Ok(commits)
}

struct SiteContent {
    index_html: String,
    posts_index_html: String,
    posts_html: Vec<(String, String)>,
}

// populate_templates(&tera, &commits[..max_history], geo_data, &posts).await?;
async fn populate_templates(
    tera: &Tera,
    commits: &[RepoCommit],
    geo_data: Option<GeoData>,
    posts: &[Post],
    posts_hash: u64,
) -> Result<SiteContent> {
    dbg!(tera.get_template_names().collect::<Vec<_>>());

    let site_content = SiteContent {
        index_html: populate_index_template(tera, commits, geo_data, posts_hash).await?,
        posts_index_html: populate_post_index_template(tera, posts).await?,
        posts_html: populate_individual_post_template(tera, posts).await?,
    };

    Ok(site_content)
}

async fn populate_individual_post_template(
    tera: &Tera,
    posts: &[Post],
) -> Result<Vec<(String, String)>> {
    let mut individual_post_html = Vec::new();
    for post in posts {
        let mut context = Context::new();
        let post_html_from_md = &markdown::to_html(&post.md_content);
        context.insert("content", post_html_from_md);
        let full_post_html = tera.render("post.html", &context)?;
        individual_post_html.push((post.filename.clone(), full_post_html));
    }
    Ok(individual_post_html)
}

async fn populate_post_index_template(tera: &Tera, posts: &[Post]) -> Result<String> {
    let mut posts_index_context = Context::new();
    let ui_posts: Vec<_> = posts.iter().map(|post| UiPost::from(post)).collect();
    posts_index_context.insert("posts", &ui_posts);

    let post_html = tera.render("posts.html", &posts_index_context)?;
    Ok(post_html)
}

async fn populate_index_template(
    tera: &Tera,
    commits: &[RepoCommit],
    mut geo_data: Option<GeoData>,
    posts_hash: u64,
) -> Result<String> {
    let ui_commits: Vec<_> = commits.iter().map(|rc| UiCommit::from(rc)).collect();
    if let Some(ref mut geo_data) = geo_data {
        correct_near_home(geo_data);
    }

    let mut activity_stats = get_activity_stats(commits);
    activity_stats.geo_data = geo_data;

    let mut context = Context::new();
    context.insert("ui_commits", &ui_commits);
    context.insert("activity_stats", &activity_stats);
    context.insert("posts_hash", &posts_hash);

    let html = tera.render("index.html", &context)?;

    Ok(html)
}

// ISP goes out of Bristol, corrects for near normal location
fn correct_near_home(geo_data: &mut GeoData) {
    let state_code = geo_data.state_code.to_lowercase();
    if geo_data.city.to_lowercase() == "bristol" && (state_code == "us-tn" || state_code == "us-va")
    {
        geo_data.city = "Abingdon".to_string();
        geo_data.state_code = "US-VA".to_string();
        geo_data.state_prov = "Virginia".to_string();
    }
}

async fn write_output(content: &SiteContent, out_dir: &Path) -> Result<()> {
    if out_dir.exists() {
        async_fs::remove_dir_all(&out_dir).await?;
    }

    async_fs::write(out_dir, &content.index_html).await?;

    let posts_dir = out_dir.join("posts");
    async_fs::create_dir_all(&posts_dir).await?;
    async_fs::write(posts_dir.join("posts.html"), &content.posts_index_html).await?;

    for (filename, post_html) in content.posts_html.iter() {
        let mut post_file_path = posts_dir.join(&filename);
        post_file_path.set_extension("html");
        async_fs::write(&post_file_path, &post_html).await?;
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
    let response = client
        .get(url)
        .header(USER_AGENT, OWNER)
        .header(ACCEPT, GH_ACCEPT_JSON)
        .bearer_auth(token)
        .send()
        .await?;

    let t: Vec<T> = response.json().await?;
    Ok(t)
}

fn order_by_date_rev(first: &RepoCommit, second: &RepoCommit) -> Ordering {
    second
        .commit
        .commit
        .author
        .date
        .cmp(&first.commit.commit.author.date)
}

fn get_activity_stats(commits: &[RepoCommit]) -> ActivityStats {
    let mut date_commits = map_commits(commits);
    fill_date_gaps(&mut date_commits);

    let max = *date_commits
        .iter()
        .max_by(|c1, c2| c1.1.cmp(c2.1))
        .unwrap()
        .1;
    let coords: Vec<_> = date_commits
        .into_iter()
        .enumerate()
        .map(|(index, (_date, count))| Point {
            x: index as u32,
            y: count,
        })
        .collect();
    let len = coords.len();

    ActivityStats {
        max,
        coords,
        len,
        geo_data: None,
    }
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

    date_commits.extend(
        missing_weeks
            .into_iter()
            .map(|(week, n)| (WeekNumber(week), n)),
    );
}

/// Collects commits into <commit date> -> <commit count>.
fn map_commits(commits: &[RepoCommit]) -> BTreeMap<WeekNumber, u32> {
    let mut date_commits = BTreeMap::new();
    for rc in commits {
        let date = rc.commit.commit.author.date.date();
        let week = WeekNumber::from(date);
        date_commits
            .entry(week)
            .and_modify(|v| *v += 1)
            .or_insert_with(|| 1);
    }

    date_commits
}

#[derive(Serialize)]
struct UiPost {
    title: String,
    html_filename: String,
}
impl From<&Post> for UiPost {
    fn from(value: &Post) -> Self {
        Self {
            title: value.title.clone(),
            html_filename: format!("{}.html", value.filename),
        }
    }
}

struct Post {
    filename: String,
    title: String,
    md_content: String,
    #[allow(unused)]
    md_ast: MarkdownNode,
    post_number: u32,
}
impl Post {
    fn try_new(md_content: String, filename: String) -> Result<Self> {
        let md_ast = markdown::to_mdast(&md_content, &ParseOptions::default())
            .map_err(|msg| anyhow!("Failed to parse Markdown: {}", msg))?;

        let MarkdownNode::Root(root) = &md_ast else {
            unreachable!();
        };

        let (post_number, filename) = filename.split_once("_").ok_or_else(|| {
            anyhow!("Invalid filename format for {filename}, expected <##>_<filename>")
        })?;
        let post_number = post_number
            .parse::<u32>()
            .map_err(|_| anyhow!("Invalid post number"))?;
        let filename = filename.to_string();

        let mut title = None;
        for node in &root.children {
            match node {
                MarkdownNode::Heading(Heading {
                    depth: 1, children, ..
                }) => {
                    let title_text = children
                        .first()
                        .ok_or_else(|| anyhow!("First heading in {filename} was empty"))?;

                    let MarkdownNode::Text(Text { value, .. }) = title_text else {
                        bail!("First heading in {filename} was not a text node");
                    };

                    title = Some(value.clone());
                    break;
                }
                MarkdownNode::Html { .. } => {
                    // probably have an HTML header
                }
                other => bail!("Unexpected first node in {filename}: {other:?}"),
            }
        }

        let Some(title) = title else {
            bail!("No title found in {filename}");
        };

        Ok(Self {
            filename,
            md_content,
            md_ast,
            title,
            post_number,
        })
    }
}
impl Hash for Post {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.filename.hash(state);
        self.md_content.hash(state);
        self.title.hash(state);
        self.post_number.hash(state);
    }
}

/// Geographic data we want to reference in the UI.
#[derive(Debug, Deserialize, Serialize)]
struct GeoData {
    // like "Virginia"
    state_prov: String,
    // like "US-VA"
    state_code: String,
    // like "Alexandria"
    city: String,
}

/// Stats we want to reference in the UI.
#[derive(Debug, Serialize)]
struct ActivityStats {
    coords: Vec<Point>,
    max: u32,
    len: usize,
    geo_data: Option<GeoData>,
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
        let formatted_date = format!(
            "{}-{:02}-{:02}",
            date.year(),
            date.month() as u8,
            date.day()
        );
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
    message: Option<String>,
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
