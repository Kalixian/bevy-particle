import bevy_sprite_render::mesh2d::vertex_output::VertexOutput;

struct AdditiveMaterial {
    color: vec4<f32>,
}

@group(2) @binding(0) var<uniform> material: AdditiveMaterial;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // The pipeline's blend state computes: dst.rgb += src.rgb * src.a
    return material.color;
}
