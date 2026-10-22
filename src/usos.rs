use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::{Value, json};
use sha1::Sha1;

use crate::config::{Settings, Token};

/// A course test (the root of a Sprawdziany tree) the user can open.
#[derive(Clone)]
pub struct Course {
    pub root: u64,
    pub test_name: String,
    pub course_name: String,
    pub course_id: String,
    pub term_id: String,
    /// USOS orders terms by this key; larger is later.
    pub term_order: i64,
}

#[derive(Clone)]
pub struct Node {
    pub id: u64,
    pub parent: Option<u64>,
    pub depth: usize,
    pub name: String,
    pub kind: Kind,
    /// Whether the user may change this node's points or grades.
    pub editable: bool,
}

#[derive(Clone)]
pub enum Kind {
    Root,
    Folder,
    Task {
        min: f64,
        max: f64,
        precision: u32,
        /// Keyed by USOS user id; empty until the entries are loaded.
        points: HashMap<String, PointEntry>,
    },
    Grade {
        scale: String,
        grades: HashMap<String, GradeEntry>,
    },
}

#[derive(Clone, Default)]
pub struct PointEntry {
    pub points: Option<f64>,
    pub comment: Option<String>,
    pub last_changed: Option<String>,
    pub grader: Option<String>,
}

#[derive(Clone, Default)]
pub struct GradeEntry {
    pub value: Option<GradeValue>,
    /// What the node's grading algorithm computed, if it has one.
    pub automatic: Option<String>,
    pub comment: Option<String>,
    pub private_comment: Option<String>,
    pub last_changed: Option<String>,
    pub grader: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct GradeValue {
    pub order_key: i64,
    pub symbol: String,
}

#[derive(Clone)]
pub struct ScaleValue {
    pub order_key: i64,
    pub symbol: String,
    pub passes: bool,
}

#[derive(Clone)]
pub struct Person {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
}

#[derive(Clone)]
pub struct Group {
    pub number: u64,
    pub class_type: String,
    pub people: Vec<String>,
}

/// A whole course test: nodes in display order, and every student it lists.
#[derive(Clone)]
pub struct Tree {
    pub nodes: Vec<Node>,
    pub students: Vec<Person>,
    /// False for the quick structure-only load; entries and students are empty then.
    pub loaded: bool,
}

pub struct NewPoints {
    pub student: String,
    pub points: f64,
    pub comment: String,
    pub last_changed: Option<String>,
}

pub struct NewGrade {
    pub student: String,
    /// The scale value's order key, or None to leave the grade empty.
    pub order_key: Option<i64>,
    pub comment: String,
    pub private_comment: String,
    pub last_changed: Option<String>,
}

#[derive(Clone)]
pub struct Usos(Arc<Backend>);

enum Backend {
    Real {
        http: reqwest::Client,
        settings: Settings,
        token: Mutex<Option<Token>>,
    },
    Mock(Mutex<Mock>),
}

impl Usos {
    pub fn real(settings: Settings, token: Option<Token>) -> Self {
        Self(Arc::new(Backend::Real {
            http: reqwest::Client::new(),
            settings,
            token: Mutex::new(token),
        }))
    }

    pub fn mock() -> Self {
        Self(Arc::new(Backend::Mock(Mutex::new(Mock::new()))))
    }

    pub fn needs_auth(&self) -> bool {
        match &*self.0 {
            Backend::Real { token, .. } => token.lock().unwrap().is_none(),
            Backend::Mock(_) => false,
        }
    }

    pub fn is_mock(&self) -> bool {
        matches!(&*self.0, Backend::Mock(_))
    }

    fn language(&self) -> &str {
        match &*self.0 {
            Backend::Real { settings, .. } => &settings.language,
            Backend::Mock(_) => "pl",
        }
    }

    /// Starts the OAuth dance; returns the URL the user must open and the request token.
    pub async fn request_token(&self) -> Result<(String, Token)> {
        let Backend::Real { http, settings, .. } = &*self.0 else {
            bail!("mock backend needs no authorization");
        };
        let body = signed_post(
            http,
            settings,
            "services/oauth/request_token",
            vec![
                ("oauth_callback".into(), "oob".into()),
                (
                    "scopes".into(),
                    // `studies` is what USOS asks for before listing group members.
                    "staff_perspective|crstests|studies|offline_access".into(),
                ),
            ],
            None,
        )
        .await?;
        let form = parse_form(&body);
        let token = Token {
            oauth_token: form
                .get("oauth_token")
                .cloned()
                .ok_or_else(|| anyhow!(body.clone()))?,
            oauth_token_secret: form.get("oauth_token_secret").cloned().unwrap_or_default(),
        };
        let url = format!(
            "{}/services/oauth/authorize?oauth_token={}",
            settings.base_url.trim_end_matches('/'),
            token.oauth_token
        );
        Ok((url, token))
    }

    pub async fn access_token(&self, request: &Token, pin: &str) -> Result<Token> {
        let Backend::Real {
            http,
            settings,
            token,
        } = &*self.0
        else {
            bail!("mock backend needs no authorization");
        };
        let body = signed_post(
            http,
            settings,
            "services/oauth/access_token",
            vec![("oauth_verifier".into(), pin.trim().into())],
            Some(request),
        )
        .await?;
        let form = parse_form(&body);
        let access = Token {
            oauth_token: form
                .get("oauth_token")
                .cloned()
                .ok_or_else(|| anyhow!(body.clone()))?,
            oauth_token_secret: form.get("oauth_token_secret").cloned().unwrap_or_default(),
        };
        *token.lock().unwrap() = Some(access.clone());
        Ok(access)
    }

    /// Every course test the user can open, latest term first.
    pub async fn courses(&self) -> Result<Vec<Course>> {
        if let Backend::Mock(m) = &*self.0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(m.lock().unwrap().courses());
        }
        let v = self.call("services/crstests/lecturer", &[]).await?;
        let lang = self.language();
        let mut courses = Vec::new();
        for (term, tests) in v["tests"].as_object().into_iter().flatten() {
            let term_info = &v["terms"][term];
            for t in tests.as_object().into_iter().flat_map(|o| o.values()) {
                let edition = &t["course_edition"];
                courses.push(Course {
                    root: t["root_id"].as_u64().or(t["node_id"].as_u64()).unwrap_or(0),
                    test_name: lang_of(&t["name"], lang),
                    course_name: lang_of(&edition["course_name"], lang),
                    course_id: str_of(&edition["course_id"]),
                    term_id: term.clone(),
                    term_order: term_info["order_key"].as_i64().unwrap_or(0),
                });
            }
        }
        courses.sort_by(|a, b| {
            b.term_order
                .cmp(&a.term_order)
                .then_with(|| a.course_name.cmp(&b.course_name))
                .then_with(|| a.test_name.cmp(&b.test_name))
        });
        Ok(courses)
    }

    /// The whole tree under `root`. Without `entries` the reply is quick; USOS spends a
    /// flat ~10 s on any request that includes students' points or grades, however
    /// many nodes it covers, so they come in one request for the whole tree.
    pub async fn tree(&self, root: u64, entries: bool) -> Result<Tree> {
        if let Backend::Mock(m) = &*self.0 {
            tokio::time::sleep(Duration::from_millis(if entries { 1500 } else { 200 })).await;
            let tree = m.lock().unwrap().tree.clone();
            return Ok(if entries { tree } else { structure_only(tree) });
        }
        let student = "student[id|first_name|last_name]|grader[first_name|last_name]";
        let (points, grades) = if entries {
            (
                format!("|all_students_points[points|comment|last_changed|{student}]"),
                format!(
                    "|all_students_grades[grade_value[order_key|symbol]|automatic_grade_value[symbol]|\
                     comment|private_comment|last_changed|{student}]"
                ),
            )
        } else {
            Default::default()
        };
        // A bracketed selector on subnodes_deep silently drops fields; without one,
        // subnodes carry the same fields as the root.
        let fields = format!(
            "id|type|name|order|parent_id|my_permissions|\
             task_node_details[points_min|points_max|points_precision{points}]|\
             grade_node_details[grade_type{grades}]|subnodes_deep"
        );
        let v = self
            .call(
                "services/crstests/node2",
                &[("node_id", root.to_string()), ("fields", fields)],
            )
            .await?;
        Ok(parse_tree(&v, entries, self.language()))
    }

    /// The values a grade scale allows, best first.
    pub async fn scale(&self, id: &str) -> Result<Vec<ScaleValue>> {
        if let Backend::Mock(_) = &*self.0 {
            return Ok(mock_scale());
        }
        let v = self
            .call(
                "services/grades/grade_type",
                &[("grade_type_id", id.into()), ("fields", "id|values".into())],
            )
            .await?;
        let mut values: Vec<ScaleValue> = v["values"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| ScaleValue {
                order_key: g["order_key"].as_i64().unwrap_or(0),
                symbol: str_of(&g["symbol"]),
                passes: g["passes"].as_bool().unwrap_or(false),
            })
            .collect();
        values.sort_by_key(|g| g.order_key);
        Ok(values)
    }

    /// The classes of the course the user teaches, with their students' ids. Other
    /// lecturers' groups are left out: USOS denies their participant lists.
    pub async fn groups(&self, course: &Course) -> Result<Vec<Group>> {
        if let Backend::Mock(m) = &*self.0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(m.lock().unwrap().groups());
        }
        let v = self
            .call(
                "services/groups/lecturer",
                &[(
                    "fields",
                    "group_number|course_id|class_type_id|class_type|participants".into(),
                )],
            )
            .await?;
        let lang = self.language();
        let mut groups: Vec<(bool, Group)> = v["groups"][course.term_id.as_str()]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|g| g["course_id"] == course.course_id.as_str())
            .map(|g| {
                let group = Group {
                    number: g["group_number"].as_u64().unwrap_or(0),
                    class_type: lang_of(&g["class_type"], lang),
                    people: g["participants"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| str_of(&p["id"]))
                        .collect(),
                };
                (g["class_type_id"] != "WYK", group)
            })
            .collect();
        // Lectures last: they hold everyone, which "all students" already covers.
        groups.sort_by_key(|(not_lecture, g)| (!*not_lecture, g.class_type.clone(), g.number));
        Ok(groups.into_iter().map(|(_, g)| g).collect())
    }

    /// Writes several students' points in one request; USOS applies all or none.
    pub async fn set_points(&self, node: u64, updates: Vec<NewPoints>) -> Result<()> {
        if let Backend::Mock(m) = &*self.0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            return m.lock().unwrap().set_points(node, updates);
        }
        let new_points: serde_json::Map<String, Value> = updates
            .into_iter()
            .map(|u| {
                // The apiref names the comment field public_comment, but USOS accepts that
                // and drops it; `comment`, as in TestPointsObject, is what gets stored.
                // A null comment makes USOS fail with a 500; clearing takes an empty string.
                let entry = json!({
                    "points": u.points,
                    "comment": u.comment,
                    "last_changed": u.last_changed.map(whole_seconds),
                });
                (u.student, entry)
            })
            .collect();
        self.confirm(
            "services/crstests/update_points",
            node,
            "new_points",
            new_points,
        )
        .await
    }

    pub async fn set_grades(&self, node: u64, updates: Vec<NewGrade>) -> Result<()> {
        if let Backend::Mock(m) = &*self.0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            return m.lock().unwrap().set_grades(node, updates);
        }
        let new_grades: serde_json::Map<String, Value> = updates
            .into_iter()
            .map(|u| {
                // update_points stores only `comment`, though its docs say public_comment;
                // this method's docs say the same, so both names carry the comment.
                let entry = json!({
                    "order": u.order_key,
                    "comment": u.comment,
                    "public_comment": u.comment,
                    "private_comment": u.private_comment,
                    "last_changed": u.last_changed.map(whole_seconds),
                });
                (u.student, entry)
            })
            .collect();
        self.confirm(
            "services/crstests/update_grades",
            node,
            "new_grades",
            new_grades,
        )
        .await
    }

    async fn confirm(
        &self,
        method: &str,
        node: u64,
        param: &str,
        updates: serde_json::Map<String, Value>,
    ) -> Result<()> {
        let v = self
            .call(
                method,
                &[
                    ("node_id", node.to_string()),
                    (param, Value::Object(updates).to_string()),
                ],
            )
            .await?;
        if v["success"] != true {
            bail!("USOS did not confirm the update: {v}");
        }
        Ok(())
    }

    async fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value> {
        let Backend::Real {
            http,
            settings,
            token,
        } = &*self.0
        else {
            unreachable!("mock calls are answered before reaching the network")
        };
        let token = token.lock().unwrap().clone();
        let params = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let body = signed_post(http, settings, method, params, token.as_ref()).await?;
        serde_json::from_str(&body).with_context(|| format!("{method}: unexpected response"))
    }
}

/// USOS hands out last_changed with microseconds but compares it at whole seconds;
/// echoing it verbatim is always reported as a concurrent change.
fn whole_seconds(t: String) -> String {
    match t.split_once('.') {
        Some((seconds, _)) => seconds.to_string(),
        None => t,
    }
}

fn person(v: &Value) -> Option<Person> {
    Some(Person {
        id: v["id"].as_str()?.to_string(),
        first_name: str_of(&v["first_name"]),
        last_name: str_of(&v["last_name"]),
    })
}

fn grader(v: &Value) -> Option<String> {
    v.as_object()
        .map(|g| format!("{} {}", str_of(&g["first_name"]), str_of(&g["last_name"])))
}

fn parse_tree(root: &Value, entries: bool, lang: &str) -> Tree {
    let mut students: BTreeMap<String, Person> = BTreeMap::new();
    let mut parse = |v: &Value| -> Node {
        let can = |perm: &str| {
            v["my_permissions"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|p| p["name"] == perm)
        };
        let kind = match v["type"].as_str().unwrap_or("") {
            "root" => Kind::Root,
            "task" => {
                let d = &v["task_node_details"];
                let mut points = HashMap::new();
                for p in d["all_students_points"].as_array().into_iter().flatten() {
                    let Some(who) = person(&p["student"]) else {
                        continue;
                    };
                    points.insert(
                        who.id.clone(),
                        PointEntry {
                            points: p["points"].as_f64(),
                            comment: p["comment"].as_str().map(str::to_string),
                            last_changed: p["last_changed"].as_str().map(str::to_string),
                            grader: grader(&p["grader"]),
                        },
                    );
                    students.entry(who.id.clone()).or_insert(who);
                }
                Kind::Task {
                    min: d["points_min"].as_f64().unwrap_or(0.0),
                    max: d["points_max"].as_f64().unwrap_or(0.0),
                    precision: d["points_precision"].as_u64().unwrap_or(1) as u32,
                    points,
                }
            }
            "grade" => {
                let d = &v["grade_node_details"];
                let mut grades = HashMap::new();
                for g in d["all_students_grades"].as_array().into_iter().flatten() {
                    let Some(who) = person(&g["student"]) else {
                        continue;
                    };
                    let value = g["grade_value"].as_object().map(|gv| GradeValue {
                        order_key: gv["order_key"].as_i64().unwrap_or(0),
                        symbol: str_of(&gv["symbol"]),
                    });
                    grades.insert(
                        who.id.clone(),
                        GradeEntry {
                            value,
                            automatic: g["automatic_grade_value"]["symbol"]
                                .as_str()
                                .map(str::to_string),
                            comment: g["comment"].as_str().map(str::to_string),
                            private_comment: g["private_comment"].as_str().map(str::to_string),
                            last_changed: g["last_changed"].as_str().map(str::to_string),
                            grader: grader(&g["grader"]),
                        },
                    );
                    students.entry(who.id.clone()).or_insert(who);
                }
                Kind::Grade {
                    scale: str_of(&d["grade_type"]["id"]),
                    grades,
                }
            }
            _ => Kind::Folder,
        };
        Node {
            id: v["id"].as_u64().unwrap_or(0),
            parent: v["parent_id"].as_u64(),
            depth: 0,
            name: lang_of(&v["name"], lang),
            editable: can("mod_points") || can("mod_grades"),
            kind,
        }
    };
    // Each node's subnodes_deep holds only its own children, nested further down.
    let mut pending = vec![root];
    let mut seen = std::collections::HashSet::new();
    let mut all: Vec<(i64, Node)> = Vec::new();
    while let Some(v) = pending.pop() {
        pending.extend(v["subnodes_deep"].as_array().into_iter().flatten());
        if seen.insert(v["id"].as_u64()) {
            all.push((v["order"].as_i64().unwrap_or(0), parse(v)));
        }
    }
    all.sort_by_key(|(order, n)| (*order, n.id));
    let nodes = order_depth_first(all.into_iter().map(|(_, n)| n).collect());
    let mut students: Vec<Person> = students.into_values().collect();
    sort_people(&mut students);
    Tree {
        nodes,
        students,
        loaded: entries,
    }
}

/// Arranges nodes parent-before-children, siblings in their USOS order (the input is
/// already sorted by it), and records each node's depth.
fn order_depth_first(nodes: Vec<Node>) -> Vec<Node> {
    let ids: std::collections::HashSet<u64> = nodes.iter().map(|n| n.id).collect();
    let mut children: HashMap<Option<u64>, Vec<Node>> = HashMap::new();
    for n in nodes {
        // A parent missing from the reply would orphan the subtree; show it at the top.
        let parent = n.parent.filter(|p| ids.contains(p));
        children.entry(parent).or_default().push(n);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(Node, usize)> = children
        .remove(&None)
        .unwrap_or_default()
        .into_iter()
        .rev()
        .map(|n| (n, 0))
        .collect();
    while let Some((mut n, depth)) = stack.pop() {
        n.depth = depth;
        if let Some(kids) = children.remove(&Some(n.id)) {
            stack.extend(kids.into_iter().rev().map(|k| (k, depth + 1)));
        }
        out.push(n);
    }
    out
}

pub fn sort_people(people: &mut [Person]) {
    people.sort_by_cached_key(|p| {
        deunicode::deunicode(&format!("{} {}", p.last_name, p.first_name)).to_lowercase()
    });
}

fn structure_only(mut tree: Tree) -> Tree {
    for n in &mut tree.nodes {
        match &mut n.kind {
            Kind::Task { points, .. } => points.clear(),
            Kind::Grade { grades, .. } => grades.clear(),
            _ => {}
        }
    }
    tree.students.clear();
    tree.loaded = false;
    tree
}

async fn signed_post(
    http: &reqwest::Client,
    settings: &Settings,
    method: &str,
    mut params: Vec<(String, String)>,
    token: Option<&Token>,
) -> Result<String> {
    let url = format!("{}/{}", settings.base_url.trim_end_matches('/'), method);
    params.extend([
        ("oauth_consumer_key".into(), settings.consumer_key.clone()),
        ("oauth_nonce".into(), nonce()),
        ("oauth_signature_method".into(), "HMAC-SHA1".into()),
        ("oauth_timestamp".into(), unix_now().to_string()),
        ("oauth_version".into(), "1.0".into()),
    ]);
    if let Some(t) = token {
        params.push(("oauth_token".into(), t.oauth_token.clone()));
    }
    let token_secret = token.map(|t| t.oauth_token_secret.as_str()).unwrap_or("");
    let signature = sign(
        "POST",
        &url,
        &params,
        &settings.consumer_secret,
        token_secret,
    );
    params.push(("oauth_signature".into(), signature));

    let resp = http
        .post(&url)
        .form(&params)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .with_context(|| format!("{method}: request failed"))?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| {
                if body.trim_start().starts_with('<') {
                    "the server failed while handling the request".into()
                } else {
                    body
                }
            });
        bail!("USOS {}: {message}", status.as_u16());
    }
    Ok(body)
}

// RFC 5849 §3.6: everything but unreserved characters is percent-encoded.
const OAUTH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn enc(s: &str) -> String {
    utf8_percent_encode(s, OAUTH).to_string()
}

fn sign(
    method: &str,
    url: &str,
    params: &[(String, String)],
    consumer: &str,
    token: &str,
) -> String {
    let mut pairs: Vec<(String, String)> = params.iter().map(|(k, v)| (enc(k), enc(v))).collect();
    pairs.sort();
    let normalized = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let base = format!("{method}&{}&{}", enc(url), enc(&normalized));
    let mut mac =
        Hmac::<Sha1>::new_from_slice(format!("{}&{}", enc(consumer), enc(token)).as_bytes())
            .expect("HMAC accepts any key length");
    mac.update(base.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

fn nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_form(body: &str) -> HashMap<String, String> {
    let dec = |s: &str| {
        percent_decode_str(&s.replace('+', " "))
            .decode_utf8_lossy()
            .into_owned()
    };
    body.split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (dec(k), dec(v)))
        .collect()
}

fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// USOS LangDicts are often filled in only one language, whichever the author used.
fn lang_of(v: &Value, preferred: &str) -> String {
    let other = if preferred == "en" { "pl" } else { "en" };
    [preferred, other]
        .iter()
        .filter_map(|k| v[k].as_str())
        .find(|s| !s.is_empty())
        .unwrap_or("?")
        .to_string()
}

fn mock_scale() -> Vec<ScaleValue> {
    [
        ("5", true),
        ("4,5", true),
        ("4", true),
        ("3,5", true),
        ("3", true),
        ("2", false),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (symbol, passes))| ScaleValue {
        order_key: i as i64 + 1,
        symbol: symbol.into(),
        passes,
    })
    .collect()
}

/// In-memory stand-in for USOS, used by `--mock` so the interface can be tried
/// without an account. Every name in it is made up, the points are random on each
/// start, and nothing is kept after the program exits.
struct Mock {
    tree: Tree,
    clock: u64,
}

/// xorshift64*, enough to vary the made-up data between runs without a dependency.
struct Rng(u64);

impl Rng {
    fn seeded() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

impl Mock {
    fn new() -> Self {
        // (first name, takes the feminine surname form)
        const FIRST: [(&str, bool); 16] = [
            ("Anna", true),
            ("Piotr", false),
            ("Zofia", true),
            ("Jakub", false),
            ("Maja", true),
            ("Kacper", false),
            ("Julia", true),
            ("Michał", false),
            ("Hanna", true),
            ("Antoni", false),
            ("Lena", true),
            ("Filip", false),
            ("Oliwia", true),
            ("Mikołaj", false),
            ("Alicja", true),
            ("Tymon", false),
        ];
        const LAST: [(&str, &str); 12] = [
            ("Lewandowski", "Lewandowska"),
            ("Wiśniewski", "Wiśniewska"),
            ("Kowalczyk", "Kowalczyk"),
            ("Zieliński", "Zielińska"),
            ("Wójcik", "Wójcik"),
            ("Kamiński", "Kamińska"),
            ("Szymański", "Szymańska"),
            ("Dąbrowski", "Dąbrowska"),
            ("Mazur", "Mazur"),
            ("Krawczyk", "Krawczyk"),
            ("Pawłowski", "Pawłowska"),
            ("Grabowski", "Grabowska"),
        ];
        const COMMENTS: [&str; 6] = [
            "Dobra robota",
            "Brak testów jednostkowych",
            "Oddane po terminie",
            "Ostrzeżenia clippy do poprawy",
            "Bardzo czytelny kod",
            "Brakuje obsługi błędów",
        ];
        let mut rng = Rng::seeded();
        let mut students: Vec<Person> = Vec::new();
        while students.len() < 24 {
            let (first, feminine) = *rng.pick(&FIRST);
            let (m, f) = *rng.pick(&LAST);
            let last = if feminine { f } else { m };
            if students
                .iter()
                .any(|p| p.first_name == first && p.last_name == last)
            {
                continue;
            }
            students.push(Person {
                id: format!("{}", 100 + students.len()),
                first_name: first.into(),
                last_name: last.into(),
            });
        }
        sort_people(&mut students);
        let node = |id, parent, name: &str, kind| Node {
            id,
            parent,
            depth: 0,
            name: name.into(),
            kind,
            editable: true,
        };
        let task = |max: f64| Kind::Task {
            min: 0.0,
            max,
            precision: 1,
            points: HashMap::new(),
        };
        let mut nodes = vec![node(1, None, "Zasady zaliczenia", Kind::Root)];
        nodes.push(node(2, Some(1), "Laboratoria", Kind::Folder));
        for i in 1..=6 {
            nodes.push(node(
                10 + i,
                Some(2),
                &format!("Laboratorium {i}"),
                task(5.0),
            ));
        }
        nodes.push(node(3, Some(1), "Kolokwia", Kind::Folder));
        nodes.push(node(21, Some(3), "Kolokwium 1", task(20.0)));
        nodes.push(node(22, Some(3), "Kolokwium 2", task(20.0)));
        nodes.push(node(4, Some(1), "Projekt", Kind::Folder));
        nodes.push(node(31, Some(4), "Projekt: etap 1", task(10.0)));
        nodes.push(node(32, Some(4), "Projekt: etap 2", task(15.0)));
        nodes.push(node(
            40,
            Some(1),
            "Ocena końcowa",
            Kind::Grade {
                scale: "STD".into(),
                grades: HashMap::new(),
            },
        ));
        // As in the middle of a term: early work mostly graded, later work barely.
        let graded_share = |id: u64| match id {
            11 => 1.0,
            12 => 0.9,
            13 => 0.6,
            14 => 0.2,
            21 => 0.95,
            31 => 0.4,
            _ => 0.0,
        };
        let scale = mock_scale();
        for n in &mut nodes {
            let share = graded_share(n.id);
            match &mut n.kind {
                Kind::Task { max, points, .. } => {
                    for s in &students {
                        if !rng.chance(share) {
                            continue;
                        }
                        let halves = (*max * 2.0) as usize;
                        // Skewed towards good marks, as real points tend to be.
                        let value = (halves - rng.below(halves / 2 + 1).min(rng.below(halves + 1)))
                            as f64
                            / 2.0;
                        points.insert(
                            s.id.clone(),
                            PointEntry {
                                points: Some(value),
                                comment: rng.chance(0.25).then(|| rng.pick(&COMMENTS).to_string()),
                                last_changed: Some("2026-10-01 10:00:00.000000".into()),
                                grader: Some("Mock Grader".into()),
                            },
                        );
                    }
                }
                Kind::Grade { grades, .. } => {
                    for s in &students {
                        if !rng.chance(0.15) {
                            continue;
                        }
                        let v = rng.pick(&scale[..5]);
                        grades.insert(
                            s.id.clone(),
                            GradeEntry {
                                value: Some(GradeValue {
                                    order_key: v.order_key,
                                    symbol: v.symbol.clone(),
                                }),
                                automatic: None,
                                comment: None,
                                private_comment: None,
                                last_changed: Some("2026-10-01 10:00:00.000000".into()),
                                grader: Some("Mock Grader".into()),
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        Self {
            tree: Tree {
                nodes: order_depth_first(nodes),
                students,
                loaded: true,
            },
            clock: 0,
        }
    }

    fn courses(&self) -> Vec<Course> {
        let course = |name: &str, id: &str, term: &str, order| Course {
            root: 1,
            test_name: "Zasady zaliczenia".into(),
            course_name: name.into(),
            course_id: id.into(),
            term_id: term.into(),
            term_order: order,
        };
        vec![
            course("Programowanie 1", "MOCK-01", "2026Z", 2),
            course("Bazy danych", "MOCK-02", "2026Z", 2),
            course("Systemy operacyjne", "MOCK-03", "2026Z", 2),
            course("Programowanie 1", "MOCK-01", "2025Z", 1),
            course("Algorytmy i struktury danych", "MOCK-04", "2025Z", 1),
        ]
    }

    fn groups(&self) -> Vec<Group> {
        let ids: Vec<String> = self.tree.students.iter().map(|s| s.id.clone()).collect();
        let half = ids.len() / 2;
        vec![
            Group {
                number: 1,
                class_type: "Laboratorium".into(),
                people: ids[..half].to_vec(),
            },
            Group {
                number: 2,
                class_type: "Laboratorium".into(),
                people: ids[half..].to_vec(),
            },
        ]
    }

    fn stamp(&mut self) -> String {
        self.clock += 1;
        format!(
            "2026-10-09 12:{:02}:{:02}.000000",
            self.clock / 60 % 60,
            self.clock % 60
        )
    }

    fn set_points(&mut self, node: u64, updates: Vec<NewPoints>) -> Result<()> {
        let stamp = self.stamp();
        let n = self.tree.nodes.iter_mut().find(|n| n.id == node);
        let Some(Kind::Task { points, .. }) = n.map(|n| &mut n.kind) else {
            bail!("no task {node}");
        };
        for u in &updates {
            let current = points.get(&u.student).and_then(|e| e.last_changed.clone());
            if current.map(whole_seconds) != u.last_changed {
                bail!("USOS 400: points were modified in the meantime");
            }
        }
        for u in updates {
            points.insert(
                u.student,
                PointEntry {
                    points: Some(u.points),
                    comment: (!u.comment.is_empty()).then_some(u.comment),
                    last_changed: Some(stamp.clone()),
                    grader: Some("Mock Grader".into()),
                },
            );
        }
        Ok(())
    }

    fn set_grades(&mut self, node: u64, updates: Vec<NewGrade>) -> Result<()> {
        let stamp = self.stamp();
        let n = self.tree.nodes.iter_mut().find(|n| n.id == node);
        let Some(Kind::Grade { grades, .. }) = n.map(|n| &mut n.kind) else {
            bail!("no grade node {node}");
        };
        for u in &updates {
            let current = grades.get(&u.student).and_then(|e| e.last_changed.clone());
            if current.map(whole_seconds) != u.last_changed {
                bail!("USOS 400: grades were modified in the meantime");
            }
        }
        let scale = mock_scale();
        for u in updates {
            let value = u.order_key.and_then(|k| {
                scale.iter().find(|v| v.order_key == k).map(|v| GradeValue {
                    order_key: k,
                    symbol: v.symbol.clone(),
                })
            });
            grades.insert(
                u.student,
                GradeEntry {
                    value,
                    automatic: None,
                    comment: (!u.comment.is_empty()).then_some(u.comment),
                    private_comment: (!u.private_comment.is_empty()).then_some(u.private_comment),
                    last_changed: Some(stamp.clone()),
                    grader: Some("Mock Grader".into()),
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Parameters and keys from RFC 5849 §3.4.1, signed with HMAC-SHA1 over its base string.
    #[test]
    fn signature_matches_reference() {
        let params = vec![
            (
                "oauth_consumer_key".to_string(),
                "9djdj82h48djs9d2".to_string(),
            ),
            ("oauth_token".to_string(), "kkk9d7dh3k39sjv7".to_string()),
            (
                "oauth_signature_method".to_string(),
                "HMAC-SHA1".to_string(),
            ),
            ("oauth_timestamp".to_string(), "137131201".to_string()),
            ("oauth_nonce".to_string(), "7d8f3e4a".to_string()),
            ("b5".to_string(), "=%3D".to_string()),
            ("a3".to_string(), "a".to_string()),
            ("c@".to_string(), String::new()),
            ("a2".to_string(), "r b".to_string()),
            ("c2".to_string(), String::new()),
            ("a3".to_string(), "2 q".to_string()),
        ];
        let sig = sign(
            "POST",
            "http://example.com/request",
            &params,
            "j49sk3j29djd",
            "dh893hdasih9",
        );
        assert_eq!(sig, "r6/TJjbCOr97/+UU0NsvSne7s5g=");
    }

    #[test]
    fn parses_nested_subnodes() {
        // Shaped like a real reply: a folder's children sit inside the folder.
        let v = json!({
            "id": 1, "type": "root", "name": {"pl": "Root"}, "order": 0, "parent_id": null,
            "subnodes_deep": [
                {"id": 2, "type": "folder", "name": {"pl": "Lab"}, "order": 0, "parent_id": 1,
                 "subnodes_deep": [
                    {"id": 4, "type": "task", "name": {"pl": "L2"}, "order": 1, "parent_id": 2,
                     "subnodes_deep": []},
                    {"id": 3, "type": "task", "name": {"pl": "L1"}, "order": 0, "parent_id": 2,
                     "subnodes_deep": []}
                 ]},
                {"id": 5, "type": "grade", "name": {"pl": "Ocena"}, "order": 1, "parent_id": 1,
                 "subnodes_deep": []}
            ]
        });
        let tree = parse_tree(&v, false, "pl");
        let got: Vec<(String, usize)> = tree
            .nodes
            .iter()
            .map(|n| (n.name.clone(), n.depth))
            .collect();
        let want = [("Root", 0), ("Lab", 1), ("L1", 2), ("L2", 2), ("Ocena", 1)];
        assert_eq!(got, want.map(|(n, d)| (n.to_string(), d)));
    }

    #[test]
    fn orders_tree_depth_first() {
        let n = |id, parent| Node {
            id,
            parent,
            depth: 0,
            name: id.to_string(),
            kind: Kind::Folder,
            editable: false,
        };
        let nodes = order_depth_first(vec![
            n(1, None),
            n(2, Some(1)),
            n(3, Some(1)),
            n(4, Some(2)),
        ]);
        let got: Vec<(u64, usize)> = nodes.iter().map(|n| (n.id, n.depth)).collect();
        assert_eq!(got, vec![(1, 0), (2, 1), (4, 2), (3, 1)]);
    }
}
