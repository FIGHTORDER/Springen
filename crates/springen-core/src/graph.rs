//! Node graph: parameter model, type registry, evaluator and serialisation.
//!
//! The evaluator is deliberately free of any UI dependency so the CLI and the
//! headless bake path stay possible.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::field::{as_color, as_gray, Field, SharedField};
use crate::project::Context;

/// Parameter kinds. `Elmos` exists as its own type because **no node parameter
/// may be expressed in pixels** — one pixel-unit parameter and the preview
/// stops matching the export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PType {
    Elmos,
    Int,
    Float,
    Bool,
    Enum,
    Color,
    /// A list of world positions, in elmos. Its own type rather than a string
    /// of numbers for the same reason `Elmos` is its own type: a coordinate
    /// that goes wrong goes wrong silently.
    Points,
    /// Free text. `Enum` cannot serve here: a raster's name is chosen when a
    /// map is imported, and the registry is built once at start-up.
    Text,
    /// An ordered brush history. Its own type for the same reason `Points` is:
    /// a stroke carries three lengths in elmos and a mode, and a stroke that
    /// goes wrong goes wrong silently.
    Strokes,
}

/// What a brush does where it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrokeMode {
    /// Add `strength` elmos of height, shaped by the falloff. Negative digs.
    /// Relative, so it survives a change further up the graph.
    Raise,
    /// Pull toward the average of the surrounding ground. Relative.
    Smooth,
    /// Pull toward `seat`, the height the terrain had here when the stroke was
    /// made. Absolute — see `Stroke::seat`.
    Level,
}

impl StrokeMode {
    pub fn key(self) -> &'static str {
        match self {
            StrokeMode::Raise => "raise",
            StrokeMode::Smooth => "smooth",
            StrokeMode::Level => "level",
        }
    }
    pub fn from_key(k: &str) -> StrokeMode {
        match k {
            "smooth" => StrokeMode::Smooth,
            "level" => StrokeMode::Level,
            _ => StrokeMode::Raise,
        }
    }
    /// Whether the stroke means something about the terrain it was drawn on,
    /// rather than only about itself.
    pub fn is_absolute(self) -> bool {
        matches!(self, StrokeMode::Level)
    }
}

/// One brush stroke. Every length is in elmos.
///
/// Recorded rather than painted into a raster, which is the whole design: a
/// stroke in elmos replays identically at a 257² preview and a 4097² bake,
/// where a painted layer is locked to whatever resolution it was painted at
/// and has to be resampled for every other one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub x: f64,
    pub z: f64,
    pub radius: f64,
    /// Elmos of height for `Raise`; a 0..1 rate for `Smooth` and `Level`.
    pub strength: f64,
    pub mode: StrokeMode,
    /// The height the terrain had here, in elmos, when the stroke was made.
    ///
    /// Only `Level` reads it, and it is what makes an absolute stroke
    /// detectably stale: change the seed of a noise node upstream and every
    /// stroke replays at the same world position on ground that is no longer
    /// the ground it was drawn on. A relative stroke does not care. An
    /// absolute one becomes nonsense, and this is how the tool can say so
    /// instead of silently levelling a hilltop to the height of a valley that
    /// used to be there.
    pub seat: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PVal {
    Num(f64),
    Bool(bool),
    Str(String),
    /// World positions in elmos, `[x, z]` each.
    Points(Vec<[f64; 2]>),
    /// An ordered brush history, replayed in order. See [`Stroke`].
    Strokes(Vec<Stroke>),
}

impl PVal {
    pub fn as_f64(&self) -> f64 {
        match self {
            PVal::Num(v) => *v,
            PVal::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            PVal::Str(s) => s.parse().unwrap_or(0.0),
            PVal::Points(p) => p.len() as f64,
            PVal::Strokes(s) => s.len() as f64,
        }
    }
    pub fn as_bool(&self) -> bool {
        match self {
            PVal::Bool(b) => *b,
            PVal::Num(v) => *v != 0.0,
            PVal::Str(s) => s == "true",
            PVal::Points(p) => !p.is_empty(),
            PVal::Strokes(s) => !s.is_empty(),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            PVal::Str(s) => s,
            _ => "",
        }
    }
    /// The points, or an empty slice for any other kind of value.
    pub fn as_points(&self) -> &[[f64; 2]] {
        match self {
            PVal::Points(p) => p,
            _ => &[],
        }
    }
    /// The stroke history, or an empty slice for any other kind of value.
    pub fn as_strokes(&self) -> &[Stroke] {
        match self {
            PVal::Strokes(s) => s,
            _ => &[],
        }
    }
    fn to_json(&self) -> Value {
        match self {
            PVal::Num(v) => json_num(*v),
            PVal::Bool(b) => Value::Bool(*b),
            PVal::Str(s) => Value::String(s.clone()),
            // Through `json_num` like every other number here: the default
            // float writer would round-trip an elmo coordinate off its grid.
            PVal::Points(p) => Value::Array(
                p.iter()
                    .map(|xz| Value::Array(vec![json_num(xz[0]), json_num(xz[1])]))
                    .collect(),
            ),
            // Wrapped in an object rather than written as a bare array so it
            // cannot be mistaken for `Points` on the way back in — an empty
            // stroke list and an empty waypoint list are both `[]`.
            PVal::Strokes(st) => {
                let mut o = serde_json::Map::new();
                o.insert(
                    "strokes".into(),
                    Value::Array(
                        st.iter()
                            .map(|k| {
                                let mut m = serde_json::Map::new();
                                m.insert("x".into(), json_num(k.x));
                                m.insert("z".into(), json_num(k.z));
                                m.insert("r".into(), json_num(k.radius));
                                m.insert("s".into(), json_num(k.strength));
                                m.insert("seat".into(), json_num(k.seat));
                                m.insert("m".into(), Value::String(k.mode.key().into()));
                                Value::Object(m)
                            })
                            .collect(),
                    ),
                );
                Value::Object(o)
            }
        }
    }
    fn from_json(v: &Value) -> Option<PVal> {
        match v {
            Value::Number(n) => n.as_f64().map(PVal::Num),
            Value::Bool(b) => Some(PVal::Bool(*b)),
            Value::String(s) => Some(PVal::Str(s.clone())),
            Value::Array(a) => Some(PVal::Points(
                a.iter()
                    .filter_map(|p| {
                        let xz = p.as_array()?;
                        Some([xz.first()?.as_f64()?, xz.get(1)?.as_f64()?])
                    })
                    .collect(),
            )),
            Value::Object(o) => Some(PVal::Strokes(
                o.get("strokes")?
                    .as_array()?
                    .iter()
                    .filter_map(|k| {
                        Some(Stroke {
                            x: k.get("x")?.as_f64()?,
                            z: k.get("z")?.as_f64()?,
                            radius: k.get("r")?.as_f64()?,
                            strength: k.get("s")?.as_f64()?,
                            seat: k.get("seat").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            mode: StrokeMode::from_key(k.get("m").and_then(|v| v.as_str())?),
                        })
                    })
                    .collect(),
            )),
            _ => None,
        }
    }
    fn signature_token(&self) -> String {
        match self {
            PVal::Num(v) => format!("{v:?}"),
            PVal::Bool(b) => b.to_string(),
            PVal::Str(s) => s.clone(),
            PVal::Points(p) => p
                .iter()
                .map(|xz| format!("{:?},{:?}", xz[0], xz[1]))
                .collect::<Vec<_>>()
                .join(";"),
            PVal::Strokes(s) => s
                .iter()
                .map(|k| {
                    format!(
                        "{:?},{:?},{:?},{:?},{:?},{}",
                        k.x,
                        k.z,
                        k.radius,
                        k.strength,
                        k.seat,
                        k.mode.key()
                    )
                })
                .collect::<Vec<_>>()
                .join(";"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub ptype: PType,
    pub def: PVal,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub options: &'static [&'static str],
}

impl ParamSpec {
    pub fn new(key: &'static str, label: &'static str, ptype: PType, def: PVal) -> ParamSpec {
        ParamSpec {
            key,
            label,
            ptype,
            def,
            min: None,
            max: None,
            options: &[],
        }
    }
    pub fn range(mut self, min: f64, max: f64) -> ParamSpec {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
    pub fn options(mut self, o: &'static [&'static str]) -> ParamSpec {
        self.options = o;
        self
    }
}

/// Shorthand mirroring the prototype's `P()`.
pub fn p_float(key: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec::new(key, label, PType::Float, PVal::Num(def)).range(min, max)
}
pub fn p_int(key: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec::new(key, label, PType::Int, PVal::Num(def)).range(min, max)
}
pub fn p_elmos(key: &'static str, label: &'static str, def: f64, min: f64, max: f64) -> ParamSpec {
    ParamSpec::new(key, label, PType::Elmos, PVal::Num(def)).range(min, max)
}
pub fn p_bool(key: &'static str, label: &'static str, def: bool) -> ParamSpec {
    ParamSpec::new(key, label, PType::Bool, PVal::Bool(def))
}
pub fn p_enum(
    key: &'static str,
    label: &'static str,
    def: &'static str,
    options: &'static [&'static str],
) -> ParamSpec {
    ParamSpec::new(key, label, PType::Enum, PVal::Str(def.into())).options(options)
}
pub fn p_text(key: &'static str, label: &'static str, def: &str) -> ParamSpec {
    ParamSpec::new(key, label, PType::Text, PVal::Str(def.into()))
}
pub fn p_color(key: &'static str, label: &'static str, def: &'static str) -> ParamSpec {
    ParamSpec::new(key, label, PType::Color, PVal::Str(def.into()))
}
pub fn p_points(key: &'static str, label: &'static str, def: Vec<[f64; 2]>) -> ParamSpec {
    ParamSpec::new(key, label, PType::Points, PVal::Points(def))
}

/// A brush history parameter. Always starts empty: a sculpt node with strokes
/// baked into its default would put someone else's hand edits on a new map.
pub fn p_strokes(key: &'static str, label: &'static str) -> ParamSpec {
    ParamSpec::new(key, label, PType::Strokes, PVal::Strokes(Vec::new()))
}

/// Live parameter values for one node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params(pub HashMap<String, PVal>);

impl Params {
    pub fn f(&self, key: &str) -> f64 {
        self.0.get(key).map(PVal::as_f64).unwrap_or(0.0)
    }
    pub fn i(&self, key: &str) -> i64 {
        self.f(key) as i64
    }
    pub fn usize(&self, key: &str) -> usize {
        self.f(key).max(0.0) as usize
    }
    pub fn b(&self, key: &str) -> bool {
        self.0.get(key).map(PVal::as_bool).unwrap_or(false)
    }
    pub fn s(&self, key: &str) -> &str {
        self.0.get(key).map(PVal::as_str).unwrap_or("")
    }
    pub fn points(&self, key: &str) -> &[[f64; 2]] {
        self.0.get(key).map(PVal::as_points).unwrap_or(&[])
    }
    pub fn strokes(&self, key: &str) -> &[Stroke] {
        self.0.get(key).map(PVal::as_strokes).unwrap_or(&[])
    }
    pub fn set(&mut self, key: &str, v: PVal) {
        self.0.insert(key.to_string(), v);
    }
}

/// Which channel layout a port or output uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chan {
    Gray,
    Color,
}

pub type Inputs = HashMap<&'static str, SharedField>;
pub type EvalFn = fn(&Inputs, &Params, &Context) -> SharedField;

#[derive(Clone)]
pub struct NodeSpec {
    pub type_name: &'static str,
    pub label: &'static str,
    pub cat: &'static str,
    pub inputs: &'static [&'static str],
    /// Ports not listed default to `Gray`.
    pub in_types: &'static [(&'static str, Chan)],
    pub produces: Chan,
    /// Terminal nodes carve out one SMF layer.
    pub output: Option<&'static str>,
    pub params: Vec<ParamSpec>,
    pub eval: EvalFn,
}

impl NodeSpec {
    pub fn is_terminal(&self) -> bool {
        self.output.is_some()
    }
    fn port_chan(&self, port: &str) -> Chan {
        self.in_types
            .iter()
            .find(|(p, _)| *p == port)
            .map(|(_, c)| *c)
            .unwrap_or(Chan::Gray)
    }
}

/// The node type registry, populated once at first use.
pub struct Registry {
    specs: HashMap<&'static str, NodeSpec>,
    order: Vec<&'static str>,
}

impl Registry {
    pub fn get(&self, type_name: &str) -> Option<&NodeSpec> {
        self.specs.get(type_name)
    }
    /// Registration order, which is also palette order.
    pub fn all(&self) -> impl Iterator<Item = &NodeSpec> {
        self.order.iter().filter_map(move |k| self.specs.get(k))
    }
    pub fn categories(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for s in self.all() {
            if !out.contains(&s.cat) {
                out.push(s.cat);
            }
        }
        out
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Collects specs during registration.
pub struct RegistryBuilder {
    specs: HashMap<&'static str, NodeSpec>,
    order: Vec<&'static str>,
}

impl RegistryBuilder {
    pub fn def(&mut self, spec: NodeSpec) {
        if !self.specs.contains_key(spec.type_name) {
            self.order.push(spec.type_name);
        }
        self.specs.insert(spec.type_name, spec);
    }
}

static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();

pub fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let mut b = RegistryBuilder {
            specs: HashMap::new(),
            order: Vec::new(),
        };
        crate::nodes::register(&mut b);
        crate::texture::register(&mut b);
        Registry {
            specs: b.specs,
            order: b.order,
        }
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub type_name: String,
    pub x: f64,
    pub y: f64,
    pub params: Params,
    /// port -> source node id
    pub inputs: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// Insertion order is preserved; terminal lookup and serialisation both
    /// depend on it.
    pub nodes: Vec<Node>,
    pub next_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    SelfLink,
    Cycle,
    NoSuchPort,
    NoSuchNode,
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::SelfLink => write!(f, "Rejected: a node cannot feed itself"),
            ConnectError::Cycle => write!(f, "Rejected: that link would create a cycle"),
            ConnectError::NoSuchPort => write!(f, "Rejected: no such input port on that node"),
            ConnectError::NoSuchNode => write!(f, "Rejected: no such node"),
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct EvalStats {
    pub hits: usize,
    pub evals: usize,
    pub per_type: HashMap<String, usize>,
}

/// Evaluation cache keyed by structural signature, so two identical subgraphs
/// are computed once.
#[derive(Default)]
pub struct Cache {
    map: HashMap<String, SharedField>,
}

impl Cache {
    pub fn clear(&mut self) {
        self.map.clear();
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Graph {
    pub fn new() -> Graph {
        Graph {
            nodes: Vec::new(),
            next_id: 1,
        }
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// Add a node with spec defaults, then apply overrides.
    pub fn add(&mut self, type_name: &str, x: f64, y: f64, overrides: &[(&str, PVal)]) -> String {
        let spec = registry()
            .get(type_name)
            .unwrap_or_else(|| panic!("Unknown node type: {type_name}"));
        let mut params = Params::default();
        for p in &spec.params {
            params.set(p.key, p.def.clone());
        }
        for (k, v) in overrides {
            params.set(k, v.clone());
        }
        let id = format!("n{}", self.next_id);
        self.next_id += 1;
        self.nodes.push(Node {
            id: id.clone(),
            type_name: type_name.to_string(),
            x,
            y,
            params,
            inputs: HashMap::new(),
        });
        id
    }

    pub fn remove(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        for n in self.nodes.iter_mut() {
            n.inputs.retain(|_, src| src != id);
        }
    }

    /// Does `from` already depend on `to`?
    pub fn would_cycle(&self, from: &str, to: &str) -> bool {
        let mut seen: Vec<&str> = Vec::new();
        let mut stack: Vec<&str> = vec![from];
        while let Some(cur) = stack.pop() {
            if cur == to {
                return true;
            }
            if seen.contains(&cur) {
                continue;
            }
            seen.push(cur);
            if let Some(n) = self.node(cur) {
                for src in n.inputs.values() {
                    stack.push(src);
                }
            }
        }
        false
    }

    pub fn connect(&mut self, src: &str, dst: &str, port: &str) -> Result<(), ConnectError> {
        if src == dst {
            return Err(ConnectError::SelfLink);
        }
        if self.node(src).is_none() || self.node(dst).is_none() {
            return Err(ConnectError::NoSuchNode);
        }
        if self.would_cycle(src, dst) {
            return Err(ConnectError::Cycle);
        }
        let type_name = self.node(dst).unwrap().type_name.clone();
        let spec = registry().get(&type_name).ok_or(ConnectError::NoSuchNode)?;
        if !spec.inputs.contains(&port) {
            return Err(ConnectError::NoSuchPort);
        }
        self.node_mut(dst)
            .unwrap()
            .inputs
            .insert(port.to_string(), src.to_string());
        Ok(())
    }

    pub fn disconnect(&mut self, dst: &str, port: &str) {
        if let Some(n) = self.node_mut(dst) {
            n.inputs.remove(port);
        }
    }

    /// Wire helper used by the starter graphs, where links are known valid.
    fn wire(&mut self, src: &str, dst: &str, port: &str) {
        self.node_mut(dst)
            .unwrap()
            .inputs
            .insert(port.to_string(), src.to_string());
    }

    pub fn link(&mut self, src: &str, dst: &str, port: &str) {
        self.wire(src, dst, port)
    }

    /// The terminal for `kind`, but only when something is wired into it.
    ///
    /// A terminal with nothing connected evaluates to a flat field, which for
    /// the heightmap means a perfectly level map and no error anywhere: the
    /// bake succeeds, the archive loads, and the terrain is a plane. Callers
    /// that need to tell "no heightmap node" from "a heightmap node nobody
    /// wired up" ask this instead of [`Graph::find_terminal`].
    pub fn find_wired_terminal(&self, kind: &str) -> Option<&str> {
        let id = self.find_terminal(kind)?;
        let node = self.node(id)?;
        if node.inputs.is_empty() {
            None
        } else {
            Some(id)
        }
    }

    pub fn find_terminal(&self, kind: &str) -> Option<&str> {
        for n in &self.nodes {
            if let Some(spec) = registry().get(&n.type_name) {
                if spec.output == Some(kind) {
                    return Some(&n.id);
                }
            }
        }
        None
    }

    /// Structural identity of a node's result: type, params, context and every
    /// upstream signature.
    pub fn signature(&self, id: &str, ctx: &Context) -> String {
        let mut seen: Vec<String> = Vec::new();
        self.signature_inner(id, ctx, &mut seen)
    }

    fn signature_inner(&self, id: &str, ctx: &Context, seen: &mut Vec<String>) -> String {
        if seen.iter().any(|s| s == id) {
            return "CYCLE".into();
        }
        seen.push(id.to_string());
        let node = match self.node(id) {
            Some(n) => n,
            None => {
                seen.pop();
                return "NULL".into();
            }
        };
        let spec = match registry().get(&node.type_name) {
            Some(s) => s,
            None => {
                seen.pop();
                return "NULL".into();
            }
        };
        let mut parts: Vec<String> = vec![
            node.type_name.clone(),
            ctx.res.to_string(),
            format!("{:?}", ctx.elmos),
            ctx.seed.to_string(),
            format!("{:?}", ctx.height_range),
        ];
        for p in &spec.params {
            let v = node
                .params
                .0
                .get(p.key)
                .cloned()
                .unwrap_or_else(|| p.def.clone());
            parts.push(format!("{}={}", p.key, v.signature_token()));
        }
        for port in spec.inputs {
            let up = node.inputs.get(*port);
            let s = match up {
                Some(u) => self.signature_inner(u, ctx, seen),
                None => "-".into(),
            };
            parts.push(format!("{port}:{s}"));
        }
        seen.pop();
        parts.join("|")
    }

    pub fn evaluate(&self, id: &str, ctx: &Context) -> SharedField {
        let mut cache = Cache::default();
        self.evaluate_cached(id, ctx, &mut cache, &mut EvalStats::default())
    }

    pub fn evaluate_cached(
        &self,
        id: &str,
        ctx: &Context,
        cache: &mut Cache,
        stats: &mut EvalStats,
    ) -> SharedField {
        let node = match self.node(id) {
            Some(n) => n,
            None => return Arc::new(Field::gray(ctx.res)),
        };
        let sig = self.signature(id, ctx);
        if let Some(hit) = cache.map.get(&sig) {
            stats.hits += 1;
            return Arc::clone(hit);
        }
        let spec = match registry().get(&node.type_name) {
            Some(s) => s,
            None => return Arc::new(Field::gray(ctx.res)),
        };
        let mut ins: Inputs = HashMap::new();
        for port in spec.inputs {
            let Some(up) = node.inputs.get(*port) else {
                continue;
            };
            let got = self.evaluate_cached(up, ctx, cache, stats);
            // Coerce channels so a colour field wired into a height port
            // becomes luminance, and a height field into a colour port
            // becomes grey.
            let coerced = match spec.port_chan(port) {
                Chan::Color => as_color(&got),
                Chan::Gray => as_gray(&got),
            };
            ins.insert(port, coerced);
        }
        let out = (spec.eval)(&ins, &node.params, ctx);
        stats.evals += 1;
        *stats.per_type.entry(node.type_name.clone()).or_insert(0) += 1;
        cache.map.insert(sig, Arc::clone(&out));
        out
    }

    /// Serialise to the prototype's project JSON shape.
    pub fn serialize(&self) -> Value {
        let mut nodes = Vec::new();
        for n in &self.nodes {
            let mut params = Map::new();
            if let Some(spec) = registry().get(&n.type_name) {
                for p in &spec.params {
                    let v = n
                        .params
                        .0
                        .get(p.key)
                        .cloned()
                        .unwrap_or_else(|| p.def.clone());
                    params.insert(p.key.to_string(), v.to_json());
                }
            }
            let mut inputs = Map::new();
            let mut ports: Vec<&String> = n.inputs.keys().collect();
            ports.sort();
            for port in ports {
                inputs.insert(port.clone(), Value::String(n.inputs[port].clone()));
            }
            let mut obj = Map::new();
            obj.insert("id".into(), Value::String(n.id.clone()));
            obj.insert("type".into(), Value::String(n.type_name.clone()));
            obj.insert("x".into(), json_num(n.x));
            obj.insert("y".into(), json_num(n.y));
            obj.insert("params".into(), Value::Object(params));
            obj.insert("inputs".into(), Value::Object(inputs));
            nodes.push(Value::Object(obj));
        }
        let mut root = Map::new();
        root.insert("version".into(), Value::from(1));
        root.insert("nextId".into(), Value::from(self.next_id));
        root.insert("nodes".into(), Value::Array(nodes));
        Value::Object(root)
    }

    pub fn deserialize(v: &Value) -> Graph {
        let mut g = Graph::new();
        if let Some(arr) = v.get("nodes").and_then(Value::as_array) {
            for nv in arr {
                let type_name = nv.get("type").and_then(Value::as_str).unwrap_or("");
                let Some(spec) = registry().get(type_name) else {
                    continue;
                };
                let mut params = Params::default();
                let pv = nv.get("params");
                for p in &spec.params {
                    let got = pv
                        .and_then(|o| o.get(p.key))
                        .and_then(PVal::from_json)
                        .unwrap_or_else(|| p.def.clone());
                    params.set(p.key, got);
                }
                let mut inputs = HashMap::new();
                if let Some(io) = nv.get("inputs").and_then(Value::as_object) {
                    for (port, src) in io {
                        if let Some(s) = src.as_str() {
                            inputs.insert(port.clone(), s.to_string());
                        }
                    }
                }
                g.nodes.push(Node {
                    id: nv
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("n0")
                        .to_string(),
                    type_name: type_name.to_string(),
                    x: nv.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                    y: nv.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                    params,
                    inputs,
                });
            }
        }
        g.next_id = v
            .get("nextId")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(g.nodes.len() as u32 + 1);
        g
    }
}

fn json_num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9e15 {
        Value::from(v as i64)
    } else {
        serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    #[test]
    fn cycles_are_refused_with_a_reason() {
        let mut g = Graph::new();
        let a = g.add("noise", 0.0, 0.0, &[]);
        let b = g.add("blur", 0.0, 0.0, &[]);
        assert!(g.connect(&a, &b, "In").is_ok());
        // Cycles are checked before ports, so feeding a generator back
        // reports the cycle rather than the missing port.
        assert_eq!(g.connect(&b, &a, "In"), Err(ConnectError::Cycle));
        let gen = g.add("noise", 0.0, 0.0, &[]);
        assert_eq!(g.connect(&a, &gen, "In"), Err(ConnectError::NoSuchPort));
        let c = g.add("blur", 0.0, 0.0, &[]);
        assert!(g.connect(&b, &c, "In").is_ok());
        assert_eq!(g.connect(&c, &b, "In"), Err(ConnectError::Cycle));
        assert_eq!(g.connect(&b, &b, "In"), Err(ConnectError::SelfLink));
    }

    #[test]
    fn round_trips_through_json() {
        let g = crate::starter::starter_graph("textured");
        let json = g.serialize();
        let back = Graph::deserialize(&json);
        assert_eq!(back.nodes.len(), g.nodes.len());
        assert_eq!(back.serialize(), json);
    }

    #[test]
    fn identical_subgraphs_share_one_evaluation() {
        let project = Project::default();
        let ctx = Context::new(&project, 33);
        let g = crate::starter::starter_graph("textured");
        let mut cache = Cache::default();
        let mut stats = EvalStats::default();
        let id = g.find_terminal("height").unwrap().to_string();
        g.evaluate_cached(&id, &ctx, &mut cache, &mut stats);
        let before = stats.evals;
        g.evaluate_cached(&id, &ctx, &mut cache, &mut stats);
        assert_eq!(stats.evals, before, "second pass must be fully cached");
        assert!(stats.hits > 0);
    }
}
