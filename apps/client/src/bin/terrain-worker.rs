#[cfg(not(target_arch = "wasm32"))]
fn main() {}

#[cfg(target_arch = "wasm32")]
fn main() {
    worker::run();
}

#[cfg(target_arch = "wasm32")]
mod worker {
    use js_sys::{Array, Float32Array, Float64Array, Uint32Array};
    use treeline_coordinates::WorldIdentity;
    use treeline_mesher::{Mesh, MeshingError};
    use treeline_voxel::{ChunkIndex, LodLevel, TransitionFaces};
    use treeline_world::{
        FarTerrainMeshSpec, FarTileIndex, GeneratedWorldTerrain, GenerationPriority,
        TerrainMeshSpec, generate_world_terrain_mesh,
    };
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    pub fn run() {
        console_error_panic_hook::set_once();
        let scope = DedicatedWorkerGlobalScope::from(JsValue::from(js_sys::global()));
        let response_scope = scope.clone();
        let mut terrain = None;
        let onmessage = Closure::wrap(Box::new(move |message: MessageEvent| {
            let request = Array::from(&message.data());
            let generated = generate_request(&request, &mut terrain);
            let (response, transfer) = encode_result(&generated);
            response_scope
                .post_message_with_transfer(&response.into(), &transfer)
                .expect("terrain worker should post its generated mesh");
        }) as Box<dyn FnMut(MessageEvent)>);
        scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        scope
            .post_message(&Array::new().into())
            .expect("terrain worker should announce readiness");
    }

    fn generate_request(
        request: &Array,
        terrain: &mut Option<GeneratedWorldTerrain>,
    ) -> treeline_world::GeneratedTerrainMesh {
        let world = WorldIdentity::new(
            parse_u64(&request.get(0)),
            parse_u32(&request.get(1)),
            parse_u64(&request.get(2)),
        );
        let priority = GenerationPriority::from_code(parse_u8(&request.get(3)))
            .expect("terrain request priority should be valid");
        let spec = match parse_u8(&request.get(4)) {
            0 => TerrainMeshSpec::Far(FarTerrainMeshSpec {
                tile: FarTileIndex::new(parse_i64(&request.get(5)), parse_i64(&request.get(6))),
            }),
            1 => TerrainMeshSpec::Near(treeline_world::ChunkMeshSpec {
                chunk: ChunkIndex::new(parse_i64(&request.get(5)), parse_i64(&request.get(6))),
                lod: LodLevel::new(parse_u8(&request.get(7))),
                transition_faces: TransitionFaces::from_bits(parse_u8(&request.get(8)))
                    .expect("terrain transition faces should be valid"),
            }),
            _ => panic!("terrain request kind should be valid"),
        };
        if terrain
            .as_ref()
            .is_none_or(|terrain| terrain.world() != world)
        {
            *terrain = Some(GeneratedWorldTerrain::new(world));
        }
        generate_world_terrain_mesh(
            terrain.as_ref().expect("terrain should be initialized"),
            priority,
            spec,
        )
    }

    fn encode_result(generated: &treeline_world::GeneratedTerrainMesh) -> (Array, Array) {
        let response = Array::new();
        let transfer = Array::new();
        response.push(&(generated.terrain_generation_time.as_secs_f64() * 1_000.0).into());
        response.push(&(generated.lake_generation_time.as_secs_f64() * 1_000.0).into());
        response.push(&encode_mesh_result(&generated.mesh, &transfer).into());
        response.push(&generated.lake_mesh.is_some().into());
        if let Some(lake_mesh) = &generated.lake_mesh {
            response.push(&encode_mesh_result(lake_mesh, &transfer).into());
        } else {
            response.push(&JsValue::NULL);
        }
        (response, transfer)
    }

    fn encode_mesh_result(result: &Result<Mesh, MeshingError>, transfer: &Array) -> Array {
        let encoded = Array::new();
        let Ok(mesh) = result else {
            encoded.push(
                &meshing_error_code(*result.as_ref().expect_err("mesh error"))
                    .to_string()
                    .into(),
            );
            return encoded;
        };
        encoded.push(&"0".into());

        let positions = Float64Array::from(flatten_dvec3(&mesh.positions).as_slice());
        let normals = Float32Array::from(flatten_vec3(&mesh.normals).as_slice());
        let colors = Float32Array::from(flatten_vec4(&mesh.colors).as_slice());
        let indices = Uint32Array::from(mesh.indices.as_slice());
        for buffer in [
            positions.buffer(),
            normals.buffer(),
            colors.buffer(),
            indices.buffer(),
        ] {
            transfer.push(&buffer);
        }
        encoded.push(&positions);
        encoded.push(&normals);
        encoded.push(&colors);
        encoded.push(&indices);
        encoded
    }

    fn flatten_vec3(values: &[[f32; 3]]) -> Vec<f32> {
        values.iter().flatten().copied().collect()
    }

    fn flatten_dvec3(values: &[[f64; 3]]) -> Vec<f64> {
        values.iter().flatten().copied().collect()
    }

    fn flatten_vec4(values: &[[f32; 4]]) -> Vec<f32> {
        values.iter().flatten().copied().collect()
    }

    const fn meshing_error_code(error: MeshingError) -> u8 {
        match error {
            MeshingError::InvalidGrid => 1,
            MeshingError::GridTooLarge => 2,
            MeshingError::MissingSurface => 3,
            MeshingError::TooManyVertices => 4,
            MeshingError::UnsupportedLod => 5,
        }
    }

    fn parse_u64(value: &JsValue) -> u64 {
        value
            .as_string()
            .and_then(|value| value.parse().ok())
            .expect("terrain request u64 should be valid")
    }

    fn parse_i64(value: &JsValue) -> i64 {
        value
            .as_string()
            .and_then(|value| value.parse().ok())
            .expect("terrain request i64 should be valid")
    }

    fn parse_u8(value: &JsValue) -> u8 {
        value
            .as_string()
            .and_then(|value| value.parse().ok())
            .expect("terrain request u8 should be valid")
    }

    fn parse_u32(value: &JsValue) -> u32 {
        value
            .as_string()
            .and_then(|value| value.parse().ok())
            .expect("terrain request u32 should be valid")
    }
}
