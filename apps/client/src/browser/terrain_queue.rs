use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::mem::size_of;
use std::rc::Rc;
use std::{cell::RefCell, error::Error};

use js_sys::Array;
use treeline_coordinates::WorldIdentity;
use treeline_mesher::Mesh;
use treeline_world::{GeneratedTerrainMesh, GenerationPriority, TerrainMeshSpec};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{Blob, BlobPropertyBag, MessageEvent, Url, Worker, window};

const TERRAIN_MESH_CACHE_BYTES: usize = 48 * 1024 * 1024;
const TERRAIN_MESH_CACHE_ENTRIES: usize = 512;
const MAX_BROWSER_TERRAIN_WORKERS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct BrowserTerrainJob {
    priority: GenerationPriority,
    sequence: u64,
    spec: TerrainMeshSpec,
}

#[derive(Debug)]
struct BrowserWorker {
    worker: Worker,
    object_url: String,
    ready: bool,
    busy: Option<BrowserTerrainJob>,
}

#[derive(Debug)]
struct WorkerEvent {
    worker_index: usize,
    data: JsValue,
}

#[derive(Clone, Debug)]
struct CachedMesh {
    generated: GeneratedTerrainMesh,
    bytes: usize,
    last_used: u64,
}

#[derive(Debug)]
struct BrowserMeshCache {
    entries: BTreeMap<TerrainMeshSpec, CachedMesh>,
    used_bytes: usize,
    clock: u64,
}

impl BrowserMeshCache {
    fn get(
        &mut self,
        spec: TerrainMeshSpec,
        priority: GenerationPriority,
    ) -> Option<GeneratedTerrainMesh> {
        self.clock = self.clock.wrapping_add(1);
        let cached = self.entries.get_mut(&spec)?;
        cached.last_used = self.clock;
        let mut generated = cached.generated.clone();
        generated.priority = priority;
        generated.terrain_generation_time = web_time::Duration::ZERO;
        generated.lake_generation_time = web_time::Duration::ZERO;
        generated.cache_hit = true;
        Some(generated)
    }

    fn contains(&self, spec: TerrainMeshSpec) -> bool {
        self.entries.contains_key(&spec)
    }

    fn insert(&mut self, generated: &GeneratedTerrainMesh) {
        if generated.mesh.is_err() || generated.lake_mesh.as_ref().is_some_and(Result::is_err) {
            return;
        }
        let bytes = generated_mesh_bytes(generated);
        if bytes > TERRAIN_MESH_CACHE_BYTES {
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        if let Some(previous) = self.entries.remove(&generated.spec) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.bytes);
        }
        self.entries.insert(
            generated.spec,
            CachedMesh {
                generated: generated.clone(),
                bytes,
                last_used: self.clock,
            },
        );
        self.used_bytes = self.used_bytes.saturating_add(bytes);

        while self.used_bytes > TERRAIN_MESH_CACHE_BYTES
            || self.entries.len() > TERRAIN_MESH_CACHE_ENTRIES
        {
            let Some((&oldest, _)) = self
                .entries
                .iter()
                .min_by_key(|(spec, entry)| (entry.last_used, *spec))
            else {
                break;
            };
            if let Some(removed) = self.entries.remove(&oldest) {
                self.used_bytes = self.used_bytes.saturating_sub(removed.bytes);
            }
        }
    }
}

/// Browser terrain queue backed by independent, message-passing Wasm workers.
#[derive(Debug)]
pub struct BrowserTerrainMeshQueue {
    world: WorldIdentity,
    workers: Vec<BrowserWorker>,
    events: Rc<RefCell<VecDeque<WorkerEvent>>>,
    pending: BinaryHeap<Reverse<BrowserTerrainJob>>,
    completed: VecDeque<GeneratedTerrainMesh>,
    cache: BrowserMeshCache,
    next_sequence: u64,
}

impl BrowserTerrainMeshQueue {
    pub fn new(world: WorldIdentity) -> Result<Self, Box<dyn Error>> {
        let navigator = window()
            .ok_or_else(|| std::io::Error::other("browser window is unavailable"))?
            .navigator();
        let worker_count = browser_worker_count(
            navigator.hardware_concurrency(),
            navigator.max_touch_points() > 0,
        );
        let events = Rc::new(RefCell::new(VecDeque::new()));
        let mut workers = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            workers.push(spawn_worker(worker_index, Rc::clone(&events))?);
        }

        Ok(Self {
            world,
            workers,
            events,
            pending: BinaryHeap::new(),
            completed: VecDeque::new(),
            cache: BrowserMeshCache {
                entries: BTreeMap::new(),
                used_bytes: 0,
                clock: 0,
            },
            next_sequence: 0,
        })
    }

    pub fn enqueue(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) {
        if let Some(generated) = self.cache.get(spec, priority) {
            self.completed.push_back(generated);
        } else {
            self.queue_if_missing(priority, spec);
        }
        self.dispatch();
    }

    pub fn prewarm(&mut self, spec: TerrainMeshSpec) -> bool {
        if self.cache.contains(spec) {
            return false;
        }
        let queued = self.queue_if_missing(GenerationPriority::PrefetchTerrain, spec);
        self.dispatch();
        queued
    }

    pub fn cancel(&mut self, spec: TerrainMeshSpec) -> bool {
        let previous_len = self.pending.len();
        self.pending.retain(|queued| queued.0.spec != spec);
        self.pending.len() != previous_len
    }

    pub fn retain_prewarm(&mut self, desired: &std::collections::BTreeSet<TerrainMeshSpec>) {
        self.pending.retain(|queued| {
            queued.0.priority != GenerationPriority::PrefetchTerrain
                || desired.contains(&queued.0.spec)
        });
    }

    pub fn try_next(&mut self) -> Option<GeneratedTerrainMesh> {
        self.receive_events();
        self.dispatch();
        self.completed.pop_front()
    }

    fn queue_if_missing(&mut self, priority: GenerationPriority, spec: TerrainMeshSpec) -> bool {
        if self
            .workers
            .iter()
            .any(|worker| worker.busy.is_some_and(|job| job.spec == spec))
        {
            return false;
        }
        if let Some(existing) = self.pending.iter().find(|queued| queued.0.spec == spec) {
            if existing.0.priority <= priority {
                return false;
            }
            self.pending.retain(|queued| queued.0.spec != spec);
        }
        self.pending.push(Reverse(BrowserTerrainJob {
            priority,
            sequence: self.next_sequence,
            spec,
        }));
        self.next_sequence = self.next_sequence.wrapping_add(1);
        true
    }

    fn receive_events(&mut self) {
        let events = self.events.borrow_mut().drain(..).collect::<Vec<_>>();
        for event in events {
            let Some(worker) = self.workers.get_mut(event.worker_index) else {
                continue;
            };
            let data = Array::from(&event.data);
            if data.length() == 0 {
                worker.ready = true;
                continue;
            }
            let Some(job) = worker.busy.take() else {
                continue;
            };
            let generated = decode_worker_result(job, &data);
            self.cache.insert(&generated);
            self.completed.push_back(generated);
        }
    }

    fn dispatch(&mut self) {
        for worker in &mut self.workers {
            if !worker.ready || worker.busy.is_some() {
                continue;
            }
            let Some(Reverse(job)) = self.pending.pop() else {
                break;
            };
            let request = encode_worker_request(self.world, job);
            if worker.worker.post_message(&request.into()).is_ok() {
                worker.busy = Some(job);
            } else {
                // The job has already left the heap, so returning it is what
                // keeps a transient post failure from silently dropping a
                // chunk and leaving a permanent hole in the world.
                self.pending.push(Reverse(job));
                break;
            }
        }
    }
}

impl Drop for BrowserTerrainMeshQueue {
    fn drop(&mut self) {
        for worker in &self.workers {
            worker.worker.terminate();
            let _ = Url::revoke_object_url(&worker.object_url);
        }
    }
}

fn spawn_worker(
    worker_index: usize,
    events: Rc<RefCell<VecDeque<WorkerEvent>>>,
) -> Result<BrowserWorker, Box<dyn Error>> {
    let base = window()
        .and_then(|window| window.document())
        .and_then(|document| document.base_uri().ok().flatten())
        .ok_or_else(|| std::io::Error::other("document base URL is unavailable"))?;
    let worker_js = js_result(
        Url::new_with_base("terrain-worker.js", &base),
        "resolve terrain worker script",
    )?
    .href();
    let worker_wasm = js_result(
        Url::new_with_base("terrain-worker_bg.wasm", &base),
        "resolve terrain worker Wasm",
    )?
    .href();
    let source = Array::new();
    source.push(&format!("importScripts(\"{worker_js}\");wasm_bindgen(\"{worker_wasm}\");").into());
    let options = BlobPropertyBag::new();
    options.set_type("text/javascript");
    let blob = js_result(
        Blob::new_with_str_sequence_and_options(&source, &options),
        "create terrain worker bootstrap",
    )?;
    let object_url = js_result(
        Url::create_object_url_with_blob(&blob),
        "create terrain worker URL",
    )?;
    let worker = js_result(Worker::new(&object_url), "start terrain worker")?;
    let onmessage = Closure::wrap(Box::new(move |message: MessageEvent| {
        events.borrow_mut().push_back(WorkerEvent {
            worker_index,
            data: message.data(),
        });
    }) as Box<dyn Fn(MessageEvent)>);
    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    Ok(BrowserWorker {
        worker,
        object_url,
        ready: false,
        busy: None,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn browser_worker_count(available: f64, touch_device: bool) -> usize {
    // Every worker owns a Wasm linear memory containing the measured bundle.
    // One worker keeps that duplication within a phone or tablet's process
    // budget; generation remains asynchronous, only less parallel.
    if touch_device || !available.is_finite() {
        return 1;
    }
    (available.max(2.0) as usize)
        .saturating_sub(1)
        .clamp(1, MAX_BROWSER_TERRAIN_WORKERS)
}

fn js_result<T>(result: Result<T, JsValue>, context: &str) -> Result<T, Box<dyn Error>> {
    result.map_err(|error| {
        Box::<dyn Error>::from(std::io::Error::other(format!("{context}: {error:?}")))
    })
}

fn encode_worker_request(world: WorldIdentity, job: BrowserTerrainJob) -> Array {
    let request = Array::new();
    request.push(&world.seed.to_string().into());
    request.push(&world.generator_version.to_string().into());
    request.push(&world.settings_hash.to_string().into());
    request.push(&job.priority.code().to_string().into());
    match job.spec {
        TerrainMeshSpec::Far(spec) => {
            request.push(&"0".into());
            request.push(&spec.tile.x.to_string().into());
            request.push(&spec.tile.z.to_string().into());
        }
        TerrainMeshSpec::Near(spec) => {
            request.push(&"1".into());
            request.push(&spec.chunk.x.to_string().into());
            request.push(&spec.chunk.z.to_string().into());
            request.push(&spec.lod.get().to_string().into());
            request.push(&spec.transition_faces.bits().to_string().into());
        }
    }
    request
}

fn decode_worker_result(job: BrowserTerrainJob, data: &Array) -> GeneratedTerrainMesh {
    GeneratedTerrainMesh {
        spec: job.spec,
        priority: job.priority,
        mesh: decode_mesh_result(&Array::from(&data.get(2))),
        lake_mesh: data
            .get(3)
            .as_bool()
            .unwrap_or(false)
            .then(|| decode_mesh_result(&Array::from(&data.get(4)))),
        terrain_generation_time: duration_from_millis(data.get(0).as_f64().unwrap_or_default()),
        lake_generation_time: duration_from_millis(data.get(1).as_f64().unwrap_or_default()),
        cache_hit: false,
    }
}

fn decode_mesh_result(data: &Array) -> Result<Mesh, treeline_mesher::MeshingError> {
    let error = data
        .get(0)
        .as_string()
        .and_then(|code| code.parse::<u8>().ok())
        .unwrap_or(1);
    if error != 0 {
        return Err(decode_meshing_error(error));
    }
    let positions = js_sys::Float64Array::new(&data.get(1)).to_vec();
    let normals = js_sys::Float32Array::new(&data.get(2)).to_vec();
    let colors = js_sys::Float32Array::new(&data.get(3)).to_vec();
    Ok(Mesh {
        positions: positions
            .chunks_exact(3)
            .map(|values| [values[0], values[1], values[2]])
            .collect(),
        normals: normals
            .chunks_exact(3)
            .map(|values| [values[0], values[1], values[2]])
            .collect(),
        colors: colors
            .chunks_exact(4)
            .map(|values| [values[0], values[1], values[2], values[3]])
            .collect(),
        indices: js_sys::Uint32Array::new(&data.get(4)).to_vec(),
    })
}

fn decode_meshing_error(code: u8) -> treeline_mesher::MeshingError {
    match code {
        2 => treeline_mesher::MeshingError::GridTooLarge,
        3 => treeline_mesher::MeshingError::MissingSurface,
        4 => treeline_mesher::MeshingError::TooManyVertices,
        5 => treeline_mesher::MeshingError::UnsupportedLod,
        _ => treeline_mesher::MeshingError::InvalidGrid,
    }
}

fn duration_from_millis(milliseconds: f64) -> web_time::Duration {
    if milliseconds.is_finite() && milliseconds >= 0.0 {
        web_time::Duration::from_secs_f64(milliseconds / 1_000.0)
    } else {
        web_time::Duration::ZERO
    }
}

fn generated_mesh_bytes(generated: &GeneratedTerrainMesh) -> usize {
    fn mesh_bytes(mesh: &Mesh) -> usize {
        mesh.positions
            .len()
            .saturating_mul(size_of::<[f64; 3]>())
            .saturating_add(mesh.normals.len().saturating_mul(size_of::<[f32; 3]>()))
            .saturating_add(mesh.colors.len().saturating_mul(size_of::<[f32; 4]>()))
            .saturating_add(mesh.indices.len().saturating_mul(size_of::<u32>()))
    }

    generated
        .mesh
        .as_ref()
        .map_or(0, mesh_bytes)
        .saturating_add(
            generated
                .lake_mesh
                .as_ref()
                .and_then(|mesh| mesh.as_ref().ok())
                .map_or(0, mesh_bytes),
        )
}
