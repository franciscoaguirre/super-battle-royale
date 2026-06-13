// Subtle CRT post-process: barrel curvature, scanlines, and a vignette.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // Barrel distortion: bend the sampling coordinates outward from the center.
    let centered = in.uv * 2.0 - 1.0;
    let dist2 = dot(centered, centered);
    let warped = centered * (1.0 + dist2 * 0.06);
    let uv = warped * 0.5 + 0.5;

    // Anything bent past the screen edge becomes the black CRT border.
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    var color = textureSample(screen_texture, texture_sampler, uv).rgb;

    // Horizontal scanlines.
    let scanline = 0.92 + 0.08 * sin(uv.y * 1400.0);
    color = color * scanline;

    // Vignette toward the edges.
    let vignette = 1.0 - dist2 * 0.2;
    color = color * vignette;

    return vec4<f32>(color, 1.0);
}
