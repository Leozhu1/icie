use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use unijudge::{
	chrono::{FixedOffset, TimeZone}, debris::{Context, Document, Find}, reqwest::{
		self, cookie_store::Cookie, header::{ORIGIN, REFERER}, Url
	}, Backend, ContestDetails, Error, Example, Language, Resource, Result, Statement, Submission, TaskDetails
};

pub struct Codeforces;

#[derive(Debug)]
pub enum TaskID {
	Normal(String),
	Zero,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
	Contest,
	Gym,
	Problemset,
}
#[derive(Debug, Clone)]
pub struct Contest {
	source: Source,
	id: String,
}
#[derive(Debug)]
pub struct Task {
	contest: Contest,
	task: TaskID,
}

#[derive(Debug)]
pub struct Session {
	client: reqwest::Client,
	username: Mutex<Option<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedAuth {
	jsessionid: Cookie<'static>,
	username: String,
}

impl unijudge::Backend for Codeforces {
	type CachedAuth = CachedAuth;
	type Contest = Contest;
	type Session = Session;
	type Task = Task;

	fn accepted_domains(&self) -> &'static [&'static str] {
		&["codeforces.com"]
	}

	fn deconstruct_resource(&self, _domain: &str, segments: &[&str]) -> Result<Resource<Self::Contest, Self::Task>> {
		let (source, contest, task) = match segments {
			["contest", contest] => return Ok(Resource::Contest(Contest { source: Source::Contest, id: (*contest).to_owned() })),
			["contest", contest, "problem", task] => (Source::Contest, contest, task),
			["gym", contest] => return Ok(Resource::Contest(Contest { source: Source::Gym, id: (*contest).to_owned() })),
			["gym", contest, "problem", task] => (Source::Gym, contest, task),
			["problemset", "problem", contest, task] => (Source::Problemset, contest, task),
			_ => return Err(Error::WrongTaskUrl),
		};
		Ok(Resource::Task(Task {
			contest: Contest { source, id: (*contest).to_owned() },
			task: if *task == "0" { TaskID::Zero } else { TaskID::Normal((*task).to_owned()) },
		}))
	}

	fn connect(&self, client: reqwest::Client, _: &str) -> Self::Session {
		Session { client, username: Mutex::new(None) }
	}

	fn auth_cache(&self, session: &Self::Session) -> Result<Option<Self::CachedAuth>> {
		let username = match session.username.lock().map_err(|_| Error::StateCorruption)?.clone() {
			Some(username) => username,
			None => return Ok(None),
		};
		let cookies = session.client.cookies().read().map_err(|_| Error::StateCorruption)?;
		let jsessionid = match cookies.0.get("codeforces.com", "/", "JSESSIONID") {
			Some(cookie) => cookie.clone().into_owned(),
			None => return Ok(None),
		};
		Ok(Some(CachedAuth { jsessionid, username }))
	}

	fn auth_deserialize(&self, data: &str) -> Result<Self::CachedAuth> {
		unijudge::deserialize_auth(data)
	}

	fn auth_login(&self, session: &Self::Session, username: &str, password: &str) -> Result<()> {
		let csrf = self.fetch_csrf(session)?;
		let mut resp = session
			.client
			.post("https://codeforces.com/enter")
			.header(ORIGIN, "https://codeforces.com")
			.header(REFERER, "https://codeforces.com/enter?back=/")
			.query(&[("back", "/")])
			.form(&[("action", "enter"), ("csrf_token", &csrf), ("handleOrEmail", username), ("password", password), ("remember", "on")])
			.send()?;
		let doc = unijudge::debris::Document::new(&resp.text()?);
		let login_succeeded =
			doc.find_all(".lang-chooser a").any(|v| v.attr("href").map(|href| href.string()).ok() == Some(format!("/profile/{}", username)));
		let wrong_password_or_handle = doc.find_all(".for__password").count() == 1;
		if login_succeeded {
			*session.username.lock().map_err(|_| Error::StateCorruption)? = Some(username.to_owned());
			Ok(())
		} else if wrong_password_or_handle {
			Err(Error::WrongCredentials)
		} else {
			Err(Error::from(doc.error("unrecognized logic outcome")))
		}
	}

	fn auth_restore(&self, session: &Self::Session, auth: &Self::CachedAuth) -> Result<()> {
		*session.username.lock().map_err(|_| Error::StateCorruption)? = Some(auth.username.clone());
		let mut cookies = session.client.cookies().write().map_err(|_| Error::StateCorruption)?;
		cookies.0.insert(auth.jsessionid.clone(), &"https://codeforces.com".parse()?).map_err(|_| Error::WrongData)?;
		Ok(())
	}

	fn auth_serialize(&self, auth: &Self::CachedAuth) -> Result<String> {
		unijudge::serialize_auth(auth)
	}

	fn task_details(&self, session: &Self::Session, task: &Self::Task) -> Result<TaskDetails> {
		let url = self.xtask_url(task)?;
		let mut resp = session.client.get(url.clone()).send()?;
		let doc = unijudge::debris::Document::new(&resp.text()?);
		let statement = if *resp.url() == url {
			ExtractedStatement::from_html(doc)?
		} else {
			let pdf = {
				let url: Url =
					format!("https://codeforces.com{}", doc.find(".datatable > div > table > tbody > tr > td > a")?.attr("href")?.string())
						.parse()?;
				let mut resp = session.client.get(url).send()?;
				let mut pdf = Vec::new();
				while resp.copy_to(&mut pdf)? > 0 {}
				pdf
			};
			let task = self
				.contest_tasks_ex(session, &task.contest)?
				.into_iter()
				.find(|t| t.symbol == self.resolve_task_id(&task))
				.ok_or_else(|| doc.error("title not found in contest task list"))?;
			ExtractedStatement { symbol: task.symbol, title: task.title, examples: None, statement: Statement::PDF { pdf } }
		};
		Ok(unijudge::TaskDetails {
			id: statement.symbol,
			title: statement.title,
			contest_id: self.pretty_contest(task),
			site_short: "codeforces".to_owned(),
			examples: statement.examples,
			statement: Some(statement.statement),
			url: url.to_string(),
		})
	}

	fn task_languages(&self, session: &Self::Session, task: &Self::Task) -> Result<Vec<Language>> {
		let url = self.task_contest_url(task)?.join("submit")?;
		let mut resp = session.client.get(url).send()?;
		if resp.url().as_str() == "https://codeforces.com/" {
			return Err(Error::AccessDenied);
		}
		let doc = unijudge::debris::Document::new(&resp.text()?);
		let languages = doc
			.find_all("[name=\"programTypeId\"] option")
			.map(|opt| Ok(unijudge::Language { id: opt.attr("value")?.as_str().trim().to_owned(), name: opt.text().string() }))
			.collect::<Result<_>>()?;
		Ok(languages)
	}

	fn task_submissions(&self, session: &Self::Session, task: &Self::Task) -> Result<Vec<Submission>> {
		let url = match task.contest.source {
			Source::Contest | Source::Gym => self.task_contest_url(task)?.join("my")?,
			Source::Problemset => format!("https://codeforces.com/submissions/{}", session.req_user()?).parse()?,
		};
		let mut resp = session.client.get(url).send()?;
		let doc = unijudge::debris::Document::new(&resp.text()?);
		Ok(doc
			.find_all("[data-submission-id]")
			.map(|node| {
				let kids = node.find_all("td").collect::<Vec<_>>();
				let id = kids[0].child(1)?.text().string();
				let verdict = if kids[5].text() == "In queue" {
					Verdict::InQueue
				} else if kids[5].text() == "Running" {
					Verdict::TestingStart
				} else {
					let verdict_span = kids[5].find_first("span")?;
					let verdict_tag = verdict_span.attr("submissionverdict")?;
					match verdict_tag.as_str() {
						"OK" => Verdict::Accepted,
						"WRONG_ANSWER" => Verdict::WrongAnswer(TestIndex::scrap(verdict_span)?),
						"COMPILATION_ERROR" => Verdict::CompilationError,
						"TESTING" => Verdict::Testing(TestIndex::scrap(verdict_span)?),
						"RUNTIME_ERROR" => Verdict::RuntimeError(TestIndex::scrap(verdict_span)?),
						"TIME_LIMIT_EXCEEDED" => Verdict::TimeLimitExceeded(TestIndex::scrap(verdict_span)?),
						"MEMORY_LIMIT_EXCEEDED" => Verdict::MemoryLimitExceeded(TestIndex::scrap(verdict_span)?),
						"PARTIAL" => Verdict::Partial(verdict_span.find(".verdict-format-points")?.text().parse()?),
						"SKIPPED" => Verdict::Skipped,
						"CHALLENGED" => Verdict::Hacked,
						_ => return Err(Error::from(verdict_span.error("unrecognized verdict tag"))),
					}
				}
				.to_unijudge();
				Ok(Submission { id, verdict })
			})
			.collect::<Result<Vec<_>>>()?)
	}

	fn task_submit(&self, session: &Self::Session, task: &Self::Task, language: &Language, code: &str) -> Result<String> {
		let url = self.task_contest_url(task)?.join("submit")?;
		let mut resp1 = session.client.get(url.clone()).send()?;
		let referer = resp1.url().clone();
		let csrf = {
			let doc = unijudge::debris::Document::new(&resp1.text()?);
			doc.find_first("[name=\"csrf_token\"]")?.attr("value")?.string()
		};
		let form = reqwest::multipart::Form::new()
			.text("csrf_token", csrf.clone())
			.text("ftaa", "")
			.text("bfaa", "")
			.text("action", "submitSolutionFormSubmitted")
			.text("contestId", task.contest.id.clone())
			.text("submittedProblemIndex", self.resolve_task_id(task).to_owned())
			.text("programTypeId", language.id.clone())
			.text("source", code.to_string())
			.text("tabSize", "4");
		session
			.client
			.post(url.clone())
			.header(ORIGIN, "https://codeforces.com")
			.header(REFERER, referer.as_str())
			.query(&[("csrf_token", &csrf)])
			.multipart(form)
			.send()?;

		Ok(self.task_submissions(session, task)?[0].id.to_string())
	}

	fn task_url(&self, _sess: &Self::Session, task: &Self::Task) -> Result<String> {
		Ok(self.xtask_url(task)?.into_string())
	}

	fn contest_id(&self, contest: &Self::Contest) -> String {
		match contest.source {
			Source::Contest => contest.id.clone(),
			Source::Gym => format!("gym{}", contest.id),
			Source::Problemset => "problemset".to_owned(),
		}
	}

	fn contest_site_prefix(&self) -> &'static str {
		"Codeforces"
	}

	fn contest_tasks(&self, session: &Self::Session, contest: &Self::Contest) -> Result<Vec<Self::Task>> {
		Ok(self
			.contest_tasks_ex(session, contest)?
			.into_iter()
			.map(|task| Task { contest: contest.clone(), task: TaskID::Normal(task.symbol) })
			.collect())
	}

	fn contest_url(&self, contest: &Self::Contest) -> String {
		match contest.source {
			Source::Contest => format!("https://codeforces.com/contest/{}/", contest.id),
			Source::Gym => format!("https://codeforces.com/gym/{}/", contest.id),
			Source::Problemset => "https://codeforces.com/problemset/".to_owned(),
		}
	}

	fn contests(&self, session: &Self::Session) -> Result<Vec<ContestDetails<Self::Contest>>> {
		let moscow_standard_time = FixedOffset::east(3 * 3600);
		let url: Url = "https://codeforces.com/contests".parse()?;
		let mut resp = session.client.get(url).send()?;
		let doc = unijudge::debris::Document::new(&resp.text()?);
		doc.find("#pageContent > .contestList")?
			.find_first(".datatable")?
			.find("table")?
			.find_all("tr[data-contestid]")
			.map(|row| {
				let id = Contest { source: Source::Contest, id: row.attr("data-contestid")?.string() };
				let title = row.find_nth("td", 0)?.text_child(0)?.string();
				let start = row.find_nth("td", 2)?.find("a")?.attr("href")?.map(|url| {
					moscow_standard_time.datetime_from_str(
						url,
						"https://www.timeanddate.com/worldclock/fixedtime.html?day=%e&month=%m&year=%Y&hour=%k&min=%M&sec=%S&p1=166",
					)
				})?;
				Ok(ContestDetails { id, title, start })
			})
			.collect()
	}

	fn name_short(&self) -> &'static str {
		"codeforces"
	}

	fn supports_contests(&self) -> bool {
		true
	}
}

pub struct ContestTaskEx {
	pub symbol: String,
	pub title: String,
}

impl Codeforces {
	pub fn contest_tasks_ex(&self, session: &Session, contest: &Contest) -> Result<Vec<ContestTaskEx>> {
		let url: Url = self.contest_url(contest).parse()?;
		let mut resp = session.client.get(url.clone()).send()?;
		if *resp.url() != url {
			return Err(Error::NotYetStarted);
		}
		let doc = unijudge::debris::Document::new(&resp.text()?);
		doc.find(".problems")?
			.find_all("tr")
			.skip(1)
			.map(|row| {
				let symbol = row.find_nth("a", 0)?.text().string();
				let title = row.find_nth("a", 1)?.text().string();
				Ok(ContestTaskEx { symbol, title })
			})
			.collect()
	}

	fn resolve_task_id<'a>(&self, task: &'a Task) -> &'a str {
		match &task.task {
			TaskID::Normal(task_id) => task_id.as_str(),
			TaskID::Zero => "A", // TODO fix https://codeforces.com/contest/1188/problem/A1
		}
	}

	fn xtask_url(&self, task: &Task) -> Result<Url> {
		let task_id = self.resolve_task_id(task);
		Ok(match task.contest.source {
			Source::Contest => format!("https://codeforces.com/contest/{}/problem/{}", task.contest.id, task_id),
			Source::Gym => format!("https://codeforces.com/gym/{}/problem/{}", task.contest.id, task_id),
			Source::Problemset => format!("https://codeforces.com/problemset/problem/{}/{}", task.contest.id, task_id),
		}
		.parse()?)
	}

	fn task_contest_url(&self, task: &Task) -> Result<Url> {
		Ok(self.contest_url(&task.contest).parse()?)
	}

	fn pretty_contest(&self, task: &Task) -> String {
		match task.contest.source {
			Source::Contest => task.contest.id.clone(),
			Source::Gym => format!("gym {}", task.contest.id),
			Source::Problemset => format!("problemset {}", task.contest.id),
		}
	}

	fn fetch_csrf(&self, session: &Session) -> Result<String> {
		let mut resp = session.client.get("https://codeforces.com").send()?;
		let doc = unijudge::debris::Document::new(&resp.text()?);
		let csrf = doc.find(".csrf-token")?.attr("data-csrf")?.string();
		Ok(csrf)
	}
}

struct ExtractedStatement {
	symbol: String,
	title: String,
	examples: Option<Vec<Example>>,
	statement: Statement,
}
impl ExtractedStatement {
	fn from_html(doc: Document) -> Result<ExtractedStatement> {
		let (symbol, title) = doc.find(".problem-statement > .header > .title")?.text().map(|full| {
			let i = match full.find('.') {
				Some(i) => i,
				None => return Err("full problem title does not have a symbol prefix"),
			};
			Ok((full[..i].trim().to_owned(), full[i + 1..].trim().to_owned()))
		})?;
		let examples = Some(
			doc.find_all(".sample-test .input")
				.zip(doc.find_all(".sample-test .output"))
				.map(|(input, output)| {
					Ok(unijudge::Example { input: input.child(1)?.text_br().string(), output: output.child(1)?.text_br().string() })
				})
				.collect::<Result<_>>()?,
		);
		let mut statement = unijudge::statement::Rewrite::start(doc);
		statement.fix_hide(|v| {
			if let unijudge::scraper::Node::Element(v) = v.value() {
				v.has_class("problem-statement", unijudge::selectors::attr::CaseSensitivity::CaseSensitive)
			} else {
				false
			}
		});
		statement.fix_override_csp();
		statement.fix_traverse(|mut v| {
			if let unijudge::scraper::Node::Element(v) = v.value() {
				unijudge::statement::fix_url(v, unijudge::qn!("href"), "//", "https:");
				unijudge::statement::fix_url(v, unijudge::qn!("src"), "//", "https:");
				unijudge::statement::fix_url(v, unijudge::qn!("href"), "/", "https://codeforces.com");
				unijudge::statement::fix_url(v, unijudge::qn!("src"), "/", "https://codeforces.com");
				if v.id() == Some("body") {
					unijudge::statement::add_style(v, "min-width: unset !important;");
				}
				if v.id() == Some("pageContent") {
					unijudge::statement::add_style(v, "margin-right: 1em !important;");
				}
			}
		});
		Ok(ExtractedStatement { symbol, title, examples, statement: statement.export() })
	}
}

impl Session {
	fn req_user(&self) -> Result<String> {
		self.username.lock().map_err(|_| Error::StateCorruption)?.clone().ok_or(Error::AccessDenied)
	}
}

#[derive(Clone, Debug)]
enum TestIndex {
	Test(i64),
	Pretest(i64),
	Hack(i64),
}
#[derive(Clone, Debug)]
enum Verdict {
	Accepted,
	MemoryLimitExceeded(TestIndex),
	WrongAnswer(TestIndex),
	TimeLimitExceeded(TestIndex),
	RuntimeError(TestIndex),
	Partial(i64),
	Testing(TestIndex),
	Hacked,
	CompilationError,
	InQueue,
	TestingStart,
	Skipped,
}

impl TestIndex {
	fn scrap(span: unijudge::debris::Node) -> unijudge::debris::Result<TestIndex> {
		let txt = span.child(0)?;
		let txt = txt.text_child(0)?;
		let num = span.find(".verdict-format-judged")?.text().parse()?;
		if txt.as_str().contains("hack") {
			Ok(TestIndex::Hack(num))
		} else if txt.as_str().contains("pretest") {
			Ok(TestIndex::Pretest(num))
		} else if txt.as_str().contains("test") {
			Ok(TestIndex::Test(num))
		} else {
			Err(txt.error("unrecognized test index"))
		}
	}

	fn desc(&self) -> String {
		match self {
			TestIndex::Test(i) => format!("test {}", i),
			TestIndex::Hack(i) => format!("hack {}", i),
			TestIndex::Pretest(i) => format!("pretest {}", i),
		}
	}
}

impl Verdict {
	fn to_unijudge(&self) -> unijudge::Verdict {
		use unijudge::{RejectionCause as UR, Verdict as UV};
		use Verdict as CV;
		match self {
			CV::Accepted => UV::Accepted,
			CV::MemoryLimitExceeded(ti) => UV::Rejected { cause: Some(UR::MemoryLimitExceeded), test: Some(ti.desc()) },
			CV::WrongAnswer(ti) => UV::Rejected { cause: Some(UR::WrongAnswer), test: Some(ti.desc()) },
			CV::TimeLimitExceeded(ti) => UV::Rejected { cause: Some(UR::TimeLimitExceeded), test: Some(ti.desc()) },
			CV::RuntimeError(ti) => UV::Rejected { cause: Some(UR::RuntimeError), test: Some(ti.desc()) },
			CV::Testing(ti) => UV::Pending { test: Some(ti.desc()) },
			CV::Partial(score) => UV::Scored { score: *score as f64, max: None, cause: None, test: None },
			CV::Hacked => UV::Rejected { cause: None, test: Some(String::from("a hack")) },
			CV::CompilationError => UV::Rejected { cause: Some(UR::CompilationError), test: None },
			CV::InQueue => UV::Pending { test: None },
			CV::TestingStart => UV::Pending { test: None },
			CV::Skipped => UV::Skipped,
		}
	}
}
