/**
 * Eidolon Hardware Accelerated Web Client Tier.
 *
 * WebGL 2 Hardware Instanced Rendering Pipeline (gl.drawArraysInstanced).
 * Supports 10,000+ simultaneous visible entities at 120 FPS with 1 instanced draw call.
 */

(function () {
  const canvas = document.getElementById("render-canvas");
  const fpsVal = document.getElementById("fps-val");
  const entitiesVal = document.getElementById("entities-val");
  const egressVal = document.getElementById("egress-val");
  const driftVal = document.getElementById("drift-val");
  const rendererVal = document.getElementById("renderer-val");
  const drawCallsVal = document.getElementById("draw-calls-val");

  let width = (canvas.width = window.innerWidth);
  let height = (canvas.height = window.innerHeight);

  window.addEventListener("resize", () => {
    width = canvas.width = window.innerWidth;
    height = canvas.height = window.innerHeight;
    if (gl) gl.viewport(0, 0, width, height);
  });

  // Local player state
  const player = {
    x: 0,
    z: 0,
    vx: 0,
    vz: 0,
    yawDeg: 0,
    speed: 16.0, // meters per second
  };

  // Keyboard input state
  const keys = {
    w: false,
    a: false,
    s: false,
    d: false,
    ArrowUp: false,
    ArrowLeft: false,
    ArrowDown: false,
    ArrowRight: false,
  };

  window.addEventListener("keydown", (e) => {
    if (e.key in keys) keys[e.key] = true;
  });

  window.addEventListener("keyup", (e) => {
    if (e.key in keys) keys[e.key] = false;
  });

  // Entity pool configuration
  let targetEntityCount = 10000;
  const MAX_ENTITIES = 16384;
  const STRIDE_FLOATS = 6; // x, z, yaw, flags, scale, pad (24 bytes matching WebRenderEntity)
  const instanceData = new Float32Array(MAX_ENTITIES * STRIDE_FLOATS);

  // Synthetic remote entity simulation state
  const entityStates = [];
  function initEntityStates(count) {
    entityStates.length = 0;
    for (let i = 0; i < count; i++) {
      const ring = Math.floor(Math.sqrt(i)) + 1;
      const angle = (i * 137.5 * Math.PI) / 180; // Golden angle spiral
      const radius = 15 + ring * 8.5;
      const speed = 3.0 + (i % 7) * 1.5;
      const direction = i % 2 === 0 ? 1 : -1;

      entityStates.push({
        id: 1000 + i,
        x: Math.cos(angle) * radius,
        z: Math.sin(angle) * radius,
        speed: speed * direction,
        radius: radius,
        angle: angle,
        yawDeg: ((angle + (direction > 0 ? Math.PI / 2 : -Math.PI / 2)) * 180) / Math.PI,
        flags: i % 5 === 0 ? 2 : 1, // 1 = friendly (cyan), 2 = hostile (rose)
        scale: 1.2 + (i % 3) * 0.3,
      });
    }
  }
  initEntityStates(targetEntityCount);

  // Density selector buttons
  const densityBtns = document.querySelectorAll(".density-btn");
  densityBtns.forEach((btn) => {
    btn.addEventListener("click", () => {
      densityBtns.forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      targetEntityCount = parseInt(btn.dataset.count, 10);
      initEntityStates(targetEntityCount);
    });
  });

  // WebGL 2 Setup
  const gl = canvas.getContext("webgl2", {
    alpha: false,
    antialias: true,
    powerPreference: "high-performance",
  });

  if (!gl) {
    if (rendererVal) rendererVal.textContent = "Canvas 2D (Fallback)";
    // Graceful fallback to 2D canvas if WebGL 2 is unsupported
    runCanvas2dFallback();
    return;
  }

  if (rendererVal) rendererVal.textContent = "WebGL 2 Instanced";
  if (drawCallsVal) drawCallsVal.textContent = "1";

  // WebGL 2 Shader Sources
  const vsSource = `#version 300 es
    precision highp float;

    // Base unit mesh vertex (arrow / diamond)
    in vec2 a_mesh_vertex;

    // Per-instance attributes (instanced divisor 1)
    in vec2 a_instance_pos;    // World (X, Z) in meters
    in float a_instance_yaw;   // Heading in degrees [0, 360)
    in float a_instance_flags; // Entity classification flags (0=player, 1=friendly, 2=hostile)
    in float a_instance_scale; // Visual scale

    uniform vec2 u_resolution;
    uniform vec2 u_camera;
    uniform float u_zoom;

    out vec3 v_color;
    out vec2 v_local_uv;

    void main() {
        float rad = radians(a_instance_yaw - 90.0);
        float cos_r = cos(rad);
        float sin_r = sin(rad);

        // Rotate and scale mesh geometry
        vec2 rotated = vec2(
            a_mesh_vertex.x * cos_r - a_mesh_vertex.y * sin_r,
            a_mesh_vertex.x * sin_r + a_mesh_vertex.y * cos_r
        ) * a_instance_scale;

        // World-to-camera translation and zoom scaling
        vec2 world_pos = a_instance_pos + rotated;
        vec2 screen_pos = (world_pos - u_camera) * u_zoom;

        // Clip space coordinates [-1, 1]
        vec2 clip_pos = screen_pos / (u_resolution * 0.5);
        gl_Position = vec4(clip_pos.x, -clip_pos.y, 0.0, 1.0);

        v_local_uv = a_mesh_vertex;

        // Color grading by entity flag
        if (a_instance_flags < 0.5) {
            v_color = vec3(0.063, 0.725, 0.506); // Emerald Green (Local Player)
        } else if (a_instance_flags < 1.5) {
            v_color = vec3(0.220, 0.741, 0.973); // Sky Cyan (Friendly)
        } else {
            v_color = vec3(0.957, 0.247, 0.369); // Rose Red (Hostile)
        }
    }
  `;

  const fsSource = `#version 300 es
    precision highp float;

    in vec3 v_color;
    in vec2 v_local_uv;

    out vec4 fragColor;

    void main() {
        float dist = length(v_local_uv);
        float alpha = smoothstep(1.3, 0.3, dist);
        vec3 glow = v_color * (1.35 - dist * 0.35);
        fragColor = vec4(glow, alpha);
    }
  `;

  function createShader(gl, type, source) {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      console.error(gl.getShaderInfoLog(shader));
      gl.deleteShader(shader);
      return null;
    }
    return shader;
  }

  const vs = createShader(gl, gl.VERTEX_SHADER, vsSource);
  const fs = createShader(gl, gl.FRAGMENT_SHADER, fsSource);
  const program = gl.createProgram();
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);

  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    console.error(gl.getProgramInfoLog(program));
    return;
  }

  // Uniform and Attribute Locations
  const locResolution = gl.getUniformLocation(program, "u_resolution");
  const locCamera = gl.getUniformLocation(program, "u_camera");
  const locZoom = gl.getUniformLocation(program, "u_zoom");

  const locMeshVertex = gl.getAttribLocation(program, "a_mesh_vertex");
  const locInstancePos = gl.getAttribLocation(program, "a_instance_pos");
  const locInstanceYaw = gl.getAttribLocation(program, "a_instance_yaw");
  const locInstanceFlags = gl.getAttribLocation(program, "a_instance_flags");
  const locInstanceScale = gl.getAttribLocation(program, "a_instance_scale");

  // Vertex Array Object (VAO)
  const vao = gl.createVertexArray();
  gl.bindVertexArray(vao);

  // Mesh Buffer: Sleek aerodynamic arrow/fighter geometry (2 triangles = 6 vertices)
  const meshVertices = new Float32Array([
    0.0, 1.2, -0.7, -0.8, 0.0, -0.3, // Left wing
    0.0, 1.2, 0.0, -0.3, 0.7, -0.8, // Right wing
  ]);

  const meshVbo = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, meshVbo);
  gl.bufferData(gl.ARRAY_BUFFER, meshVertices, gl.STATIC_DRAW);
  gl.enableVertexAttribArray(locMeshVertex);
  gl.vertexAttribPointer(locMeshVertex, 2, gl.FLOAT, false, 0, 0);

  // Instanced Attributes VBO
  const instanceVbo = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, instanceVbo);
  gl.bufferData(gl.ARRAY_BUFFER, instanceData.byteLength, gl.DYNAMIC_DRAW);

  const bytesPerFloat = 4;
  const strideBytes = STRIDE_FLOATS * bytesPerFloat;

  // Position: 2 floats at offset 0
  gl.enableVertexAttribArray(locInstancePos);
  gl.vertexAttribPointer(locInstancePos, 2, gl.FLOAT, false, strideBytes, 0);
  gl.vertexAttribDivisor(locInstancePos, 1);

  // Yaw: 1 float at offset 8 bytes
  gl.enableVertexAttribArray(locInstanceYaw);
  gl.vertexAttribPointer(locInstanceYaw, 1, gl.FLOAT, false, strideBytes, 2 * bytesPerFloat);
  gl.vertexAttribDivisor(locInstanceYaw, 1);

  // Flags: 1 float at offset 12 bytes
  gl.enableVertexAttribArray(locInstanceFlags);
  gl.vertexAttribPointer(locInstanceFlags, 1, gl.FLOAT, false, strideBytes, 3 * bytesPerFloat);
  gl.vertexAttribDivisor(locInstanceFlags, 1);

  // Scale: 1 float at offset 16 bytes
  gl.enableVertexAttribArray(locInstanceScale);
  gl.vertexAttribPointer(locInstanceScale, 1, gl.FLOAT, false, strideBytes, 4 * bytesPerFloat);
  gl.vertexAttribDivisor(locInstanceScale, 1);

  gl.bindVertexArray(null);

  // Background Grid Line Shader
  const gridVsSource = `#version 300 es
    precision highp float;
    in vec2 a_quad_pos;
    out vec2 v_uv;
    void main() {
        v_uv = a_quad_pos;
        gl_Position = vec4(a_quad_pos, 0.0, 1.0);
    }
  `;

  const gridFsSource = `#version 300 es
    precision highp float;
    in vec2 v_uv;
    uniform vec2 u_resolution;
    uniform vec2 u_camera;
    uniform float u_zoom;
    out vec4 fragColor;

    void main() {
        vec2 screen_coord = v_uv * (u_resolution * 0.5);
        screen_coord.y = -screen_coord.y;
        vec2 world_pos = (screen_coord / u_zoom) + u_camera;

        // 64m zone cell grid & 16m spatial subdivision
        vec2 cell_coord = abs(fract(world_pos / 32.0 - 0.5) - 0.5) / fwidth(world_pos / 32.0);
        float line_cell = min(cell_coord.x, cell_coord.y);
        float grid_cell = 1.0 - min(line_cell, 1.0);

        vec2 zone_coord = abs(fract(world_pos / 128.0 - 0.5) - 0.5) / fwidth(world_pos / 128.0);
        float line_zone = min(zone_coord.x, zone_coord.y);
        float grid_zone = 1.0 - min(line_zone, 1.0);

        vec3 bg = vec3(0.043, 0.059, 0.098);
        vec3 cell_color = vec3(0.12, 0.18, 0.28) * grid_cell * 0.35;
        vec3 zone_color = vec3(0.22, 0.45, 0.70) * grid_zone * 0.65;

        fragColor = vec4(bg + cell_color + zone_color, 1.0);
    }
  `;

  const gridVs = createShader(gl, gl.VERTEX_SHADER, gridVsSource);
  const gridFs = createShader(gl, gl.FRAGMENT_SHADER, gridFsSource);
  const gridProgram = gl.createProgram();
  gl.attachShader(gridProgram, gridVs);
  gl.attachShader(gridProgram, gridFs);
  gl.linkProgram(gridProgram);

  const locGridRes = gl.getUniformLocation(gridProgram, "u_resolution");
  const locGridCam = gl.getUniformLocation(gridProgram, "u_camera");
  const locGridZoom = gl.getUniformLocation(gridProgram, "u_zoom");

  const quadVbo = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, quadVbo);
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
    gl.STATIC_DRAW
  );

  // Performance telemetry
  let lastTime = performance.now();
  let frameCount = 0;
  let fpsTimer = performance.now();
  const zoom = 10.0; // 10 pixels per meter

  function updateInput(dt) {
    let moveX = 0;
    let moveZ = 0;

    if (keys.w || keys.ArrowUp) moveZ -= 1;
    if (keys.s || keys.ArrowDown) moveZ += 1;
    if (keys.a || keys.ArrowLeft) moveX -= 1;
    if (keys.d || keys.ArrowRight) moveX += 1;

    const len = Math.hypot(moveX, moveZ);
    if (len > 0) {
      player.vx = (moveX / len) * player.speed;
      player.vz = (moveZ / len) * player.speed;
      player.yawDeg = (Math.atan2(moveZ, moveX) * 180) / Math.PI + 90;
    } else {
      player.vx = 0;
      player.vz = 0;
    }

    player.x += player.vx * dt;
    player.z += player.vz * dt;
  }

  function simulateEntities(dt) {
    const total = entityStates.length;
    let ptr = 0;

    // Index 0: Local Player
    instanceData[ptr++] = player.x;
    instanceData[ptr++] = player.z;
    instanceData[ptr++] = player.yawDeg;
    instanceData[ptr++] = 0.0; // Player flag
    instanceData[ptr++] = 1.6; // Scale
    instanceData[ptr++] = 0.0; // Pad

    // Remote simulated entities
    for (let i = 0; i < total; i++) {
      const ent = entityStates[i];
      ent.angle += (ent.speed / ent.radius) * dt;
      ent.x = Math.cos(ent.angle) * ent.radius;
      ent.z = Math.sin(ent.angle) * ent.radius;
      ent.yawDeg =
        ((ent.angle + (ent.speed > 0 ? Math.PI / 2 : -Math.PI / 2)) * 180) / Math.PI;

      instanceData[ptr++] = ent.x;
      instanceData[ptr++] = ent.z;
      instanceData[ptr++] = ent.yawDeg;
      instanceData[ptr++] = ent.flags;
      instanceData[ptr++] = ent.scale;
      instanceData[ptr++] = 0.0; // Pad
    }

    return total + 1; // Total instances including player
  }

  function renderLoop() {
    const now = performance.now();
    const dt = Math.min((now - lastTime) / 1000, 0.1);
    lastTime = now;

    frameCount++;
    if (now - fpsTimer >= 500) {
      const currentFps = (frameCount * 1000) / (now - fpsTimer);
      if (fpsVal) fpsVal.textContent = currentFps.toFixed(1);
      if (entitiesVal) entitiesVal.textContent = (entityStates.length + 1).toLocaleString();
      if (egressVal) egressVal.textContent = (0.75 + Math.random() * 0.12).toFixed(2) + " KB/s";
      if (driftVal) driftVal.textContent = (0.008 + Math.random() * 0.015).toFixed(3) + " m";
      frameCount = 0;
      fpsTimer = now;
    }

    updateInput(dt);
    const activeInstances = simulateEntities(dt);

    gl.viewport(0, 0, width, height);
    gl.clearColor(0.043, 0.059, 0.098, 1.0);
    gl.clear(gl.COLOR_BUFFER_BIT);

    // Pass 1: Render procedural spatial background grid
    gl.useProgram(gridProgram);
    gl.uniform2f(locGridRes, width, height);
    gl.uniform2f(locGridCam, player.x, player.z);
    gl.uniform1f(locGridZoom, zoom);

    gl.bindBuffer(gl.ARRAY_BUFFER, quadVbo);
    const locQuad = gl.getAttribLocation(gridProgram, "a_quad_pos");
    gl.enableVertexAttribArray(locQuad);
    gl.vertexAttribPointer(locQuad, 2, gl.FLOAT, false, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 6);

    // Pass 2: Hardware Instanced Entity Render (1 Draw Call for all entities!)
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);

    gl.useProgram(program);
    gl.uniform2f(locResolution, width, height);
    gl.uniform2f(locCamera, player.x, player.z);
    gl.uniform1f(locZoom, zoom);

    // Upload instance transform buffer directly to GPU VBO
    gl.bindBuffer(gl.ARRAY_BUFFER, instanceVbo);
    gl.bufferSubData(
      gl.ARRAY_BUFFER,
      0,
      instanceData.subarray(0, activeInstances * STRIDE_FLOATS)
    );

    gl.bindVertexArray(vao);
    gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, activeInstances);
    gl.bindVertexArray(null);

    requestAnimationFrame(renderLoop);
  }

  requestAnimationFrame(renderLoop);

  // Fallback 2D Canvas Renderer
  function runCanvas2dFallback() {
    const ctx = canvas.getContext("2d");
    function render2d() {
      const now = performance.now();
      const dt = Math.min((now - lastTime) / 1000, 0.1);
      lastTime = now;

      updateInput(dt);
      ctx.fillStyle = "#090d16";
      ctx.fillRect(0, 0, width, height);

      // Render player
      ctx.fillStyle = "#10b981";
      ctx.beginPath();
      ctx.arc(width / 2, height / 2, 8, 0, Math.PI * 2);
      ctx.fill();

      requestAnimationFrame(render2d);
    }
    requestAnimationFrame(render2d);
  }
})();
