# Assets

`demos.ron` lists the demo scenes in order: number keys reach the first nine, and the left and
right arrows all of them. Each name is an effect, `effects/<name>.effect.ron`.

## Effects and emitters

Saving from the editor rewrites a file without its comments. The types are in `src/effect_file.rs`.

```text
- An effect's emitters are File("name.emitter.ron") or Inline((emitter: (..), ..)).
- An effect's optional render: (blend: Additive, softness: 1.0, stretch: 0.05,
  texture: (path: "sheet.png", columns: 4, rows: 4, playback: Fps(12.0))).
  Blends: Alpha, Additive. Playback: OverLife, Fps(..), Random.
- Emitter fields left out use `ParticleEmitter::default()`.
- An emitter's keyframes, next to `emitter`: tracks: (rate: (clock: Time, keys: [..]), ..).
  Ranges are keyed as (from, to). Only clock: Time works here. Any of rate, lifetime, speed,
  angle, radius, mass_per_radius, red, green, blue, alpha, rotation, spin, and spawn:
  Rect(half_extents: (..)), Ellipse(radii: (..)), Ring(radius: (..), arc: (..)) or
  Line(from: (..), to: (..)), matching the emitter's spawn.
- An emitter's spawn: Point, Rect(half_extents: (9.0, 9.0)), Ellipse(radii: (9.0, 9.0)),
  Ring(radius: (from: 8.0, to: 9.0), arc: (from: 0.0, to: 3.14)) or
  Line(from: (0.0, 0.0), to: (9.0, 0.0)). Rect and Ellipse take an optional fill: Edge; Ring's
  arc is optional. Its direction (what angle is measured from): Fixed, Outward, Normal.
- An effect's emitters can have a position and a rotation (radians).
  To keyframe them, next to `position`: motion: (position: (clock: Time, keys: [..]), rotation: ..).
- Random ranges: (from: 0.5, to: 2.0), optionally with shape: Low(3.0), High(3.0),
  Middle(3.0) or Falloff(6.0).
- Modifier parameters: Constant(value), or
  Track((clock: Life, keys: [(time: 0.0, value: 1.0, ease: Smooth), ...], repeat: Loop)).
  Clocks: Life, Seconds, Time. Ease (Linear, Smooth, Step) and repeat (Once, Loop, PingPong)
  are optional.
- Colors in modifiers are linear RGB; emitter red/green/blue are sRGB.
```
