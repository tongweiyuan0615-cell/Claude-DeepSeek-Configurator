const encoder = new TextEncoder();
const decoder = new TextDecoder();

const CORS_HEADERS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "GET,POST,OPTIONS",
  "Access-Control-Allow-Headers": "Authorization,Content-Type",
  "Access-Control-Max-Age": "86400",
};

const LICENSE_KEY_PREFIX = "CDSK";
const LICENSE_KEY_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

class HttpError extends Error {
  constructor(status, code, message) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export default {
  async fetch(request, env) {
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: CORS_HEADERS });
    }

    try {
      const url = new URL(request.url);
      const path = normalizePath(url.pathname);

      if (request.method === "GET" && (path === "/" || path === "/health")) {
        return json({
          ok: true,
          service: "claude-deepseek-license",
          version: "1.0.0",
        });
      }

      if (request.method === "POST" && path === "/activate") {
        return json(await activateLicense(request, env));
      }

      if (request.method === "POST" && path === "/check") {
        return json(await checkLicense(request, env));
      }

      if (path.startsWith("/admin/")) {
        await requireAdmin(request, env);

        if (request.method === "GET" && path === "/admin/licenses") {
          return json(await listLicenses(url, env));
        }

        if (request.method === "POST" && path === "/admin/licenses") {
          return json(await createLicense(request, env), 201);
        }

        if (request.method === "POST" && path === "/admin/licenses/revoke") {
          return json(await revokeLicense(request, env));
        }

        if (request.method === "POST" && path === "/admin/licenses/reset-device") {
          return json(await resetLicenseDevice(request, env));
        }
      }

      throw new HttpError(404, "NOT_FOUND", "Endpoint not found.");
    } catch (error) {
      if (error instanceof HttpError) {
        return json(
          { ok: false, error: { code: error.code, message: error.message } },
          error.status,
        );
      }

      return json(
        {
          ok: false,
          error: {
            code: "INTERNAL_ERROR",
            message: "Unexpected server error.",
          },
        },
        500,
      );
    }
  },
};

async function activateLicense(request, env) {
  const body = await readJson(request);
  const licenseKey = normalizeLicenseKey(body.license_key);
  const deviceId = normalizeRequiredString(body.device_id, "device_id");
  const platform = normalizeOptionalString(body.platform, 64);
  const appVersion = normalizeOptionalString(body.app_version, 64);

  const license = await findLicenseByKey(env, licenseKey);
  assertLicenseUsable(license);

  const now = new Date().toISOString();
  const deviceHash = await hashDeviceId(env, deviceId);
  const activation = await getActivation(env, license.id, deviceHash);

  if (activation) {
    await env.DB.prepare(
      `UPDATE device_activations
       SET last_seen_at = ?, platform = COALESCE(?, platform), app_version = COALESCE(?, app_version)
       WHERE id = ?`,
    )
      .bind(now, platform, appVersion, activation.id)
      .run();

    return activationResponse(env, license, {
      ...activation,
      platform: platform ?? activation.platform,
      app_version: appVersion ?? activation.app_version,
      last_seen_at: now,
    });
  }

  const activationCount = await countActivations(env, license.id);
  const maxDevices = getAllowedDeviceCount(license);
  if (activationCount >= maxDevices) {
    throw new HttpError(
      409,
      "DEVICE_LIMIT_REACHED",
      "This license key is already bound to another device.",
    );
  }

  const newActivation = {
    id: crypto.randomUUID(),
    license_key_id: license.id,
    device_hash: deviceHash,
    platform,
    app_version: appVersion,
    first_activated_at: now,
    last_seen_at: now,
  };

  await env.DB.prepare(
    `INSERT INTO device_activations
      (id, license_key_id, device_hash, platform, app_version, first_activated_at, last_seen_at)
     VALUES (?, ?, ?, ?, ?, ?, ?)`,
  )
    .bind(
      newActivation.id,
      newActivation.license_key_id,
      newActivation.device_hash,
      newActivation.platform,
      newActivation.app_version,
      newActivation.first_activated_at,
      newActivation.last_seen_at,
    )
    .run();

  return activationResponse(env, license, newActivation);
}

async function checkLicense(request, env) {
  const body = await readJson(request);
  const licenseToken = normalizeRequiredString(body.license_token, "license_token");
  const deviceId = normalizeRequiredString(body.device_id, "device_id");
  const platform = normalizeOptionalString(body.platform, 64);
  const appVersion = normalizeOptionalString(body.app_version, 64);

  const payload = await verifyLicenseToken(env, licenseToken);
  if (payload.typ !== "license") {
    throw new HttpError(401, "INVALID_TOKEN", "Invalid license token.");
  }

  const deviceHash = await hashDeviceId(env, deviceId);
  if (!timingSafeEqual(payload.did, deviceHash)) {
    throw new HttpError(401, "DEVICE_MISMATCH", "License token does not match this device.");
  }

  const license = await getLicenseById(env, payload.lid);
  assertLicenseUsable(license);

  const activation = await getActivation(env, license.id, deviceHash);
  if (!activation) {
    throw new HttpError(401, "DEVICE_NOT_BOUND", "This device is no longer bound to the license.");
  }

  const now = new Date().toISOString();
  await env.DB.prepare(
    `UPDATE device_activations
     SET last_seen_at = ?, platform = COALESCE(?, platform), app_version = COALESCE(?, app_version)
     WHERE id = ?`,
  )
    .bind(now, platform, appVersion, activation.id)
    .run();

  return {
    ok: true,
    license: publicLicense(license),
    activation: {
      first_activated_at: activation.first_activated_at,
      last_seen_at: now,
    },
  };
}

async function createLicense(request, env) {
  const body = await readJson(request);
  const now = new Date().toISOString();
  const licenseKey = generateLicenseKey();
  const license = {
    id: crypto.randomUUID(),
    key_hash: await hashLicenseKey(env, licenseKey),
    status: "active",
    plan: normalizeOptionalString(body.plan, 64) ?? "lifetime",
    max_devices: 1,
    expires_at: normalizeExpiresAt(body.expires_at),
    note: normalizeOptionalString(body.note, 500),
    created_at: now,
    revoked_at: null,
  };

  await env.DB.prepare(
    `INSERT INTO license_keys
      (id, key_hash, status, plan, max_devices, expires_at, note, created_at, revoked_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  )
    .bind(
      license.id,
      license.key_hash,
      license.status,
      license.plan,
      license.max_devices,
      license.expires_at,
      license.note,
      license.created_at,
      license.revoked_at,
    )
    .run();

  return {
    ok: true,
    license_key: licenseKey,
    license: publicLicense(license),
    warning: "Store this license key now. Only its hash is saved in D1.",
  };
}

async function revokeLicense(request, env) {
  const body = await readJson(request);
  const license = await resolveLicenseSelector(env, body);
  const now = new Date().toISOString();

  await env.DB.prepare(
    `UPDATE license_keys
     SET status = 'revoked', revoked_at = ?
     WHERE id = ?`,
  )
    .bind(now, license.id)
    .run();

  return {
    ok: true,
    license: publicLicense({ ...license, status: "revoked", revoked_at: now }),
  };
}

async function resetLicenseDevice(request, env) {
  const body = await readJson(request);
  const license = await resolveLicenseSelector(env, body);
  const result = await env.DB.prepare(
    "DELETE FROM device_activations WHERE license_key_id = ?",
  )
    .bind(license.id)
    .run();

  return {
    ok: true,
    license: publicLicense(license),
    reset_devices: result.meta?.changes ?? 0,
  };
}

async function listLicenses(url, env) {
  const limitParam = Number(url.searchParams.get("limit") ?? "50");
  const limit = Number.isFinite(limitParam) ? Math.min(Math.max(limitParam, 1), 200) : 50;
  const result = await env.DB.prepare(
    `SELECT
       lk.id,
       lk.status,
       lk.plan,
       lk.max_devices,
       lk.expires_at,
       lk.note,
       lk.created_at,
       lk.revoked_at,
       COUNT(da.id) AS activation_count
     FROM license_keys lk
     LEFT JOIN device_activations da ON da.license_key_id = lk.id
     GROUP BY lk.id
     ORDER BY lk.created_at DESC
     LIMIT ?`,
  )
    .bind(limit)
    .all();

  return {
    ok: true,
    licenses: result.results.map((row) => ({
      ...publicLicense(row),
      activation_count: Number(row.activation_count ?? 0),
    })),
  };
}

async function activationResponse(env, license, activation) {
  return {
    ok: true,
    license_token: await signLicenseToken(env, {
      typ: "license",
      lid: license.id,
      did: activation.device_hash,
      plan: license.plan,
      max_devices: getAllowedDeviceCount(license),
      exp: license.expires_at,
      iat: Math.floor(Date.now() / 1000),
    }),
    license: publicLicense(license),
    activation: {
      first_activated_at: activation.first_activated_at,
      last_seen_at: activation.last_seen_at,
    },
  };
}

function publicLicense(license) {
  return {
    id: license.id,
    status: license.status,
    plan: license.plan,
    max_devices: getAllowedDeviceCount(license),
    expires_at: license.expires_at ?? null,
    note: license.note ?? null,
    created_at: license.created_at ?? null,
    revoked_at: license.revoked_at ?? null,
  };
}

async function resolveLicenseSelector(env, body) {
  let license = null;
  if (body.license_id) {
    license = await getLicenseById(env, normalizeRequiredString(body.license_id, "license_id"));
  } else if (body.license_key) {
    license = await findLicenseByKey(env, normalizeLicenseKey(body.license_key));
  } else {
    throw new HttpError(400, "MISSING_LICENSE_SELECTOR", "Provide license_id or license_key.");
  }

  if (!license) {
    throw new HttpError(404, "LICENSE_NOT_FOUND", "License key was not found.");
  }

  return license;
}

async function findLicenseByKey(env, licenseKey) {
  const keyHash = await hashLicenseKey(env, licenseKey);
  return env.DB.prepare("SELECT * FROM license_keys WHERE key_hash = ? LIMIT 1")
    .bind(keyHash)
    .first();
}

async function getLicenseById(env, id) {
  return env.DB.prepare("SELECT * FROM license_keys WHERE id = ? LIMIT 1").bind(id).first();
}

async function getActivation(env, licenseId, deviceHash) {
  return env.DB.prepare(
    `SELECT *
     FROM device_activations
     WHERE license_key_id = ? AND device_hash = ?
     LIMIT 1`,
  )
    .bind(licenseId, deviceHash)
    .first();
}

async function countActivations(env, licenseId) {
  const row = await env.DB.prepare(
    "SELECT COUNT(*) AS count FROM device_activations WHERE license_key_id = ?",
  )
    .bind(licenseId)
    .first();

  return Number(row?.count ?? 0);
}

function assertLicenseUsable(license) {
  if (!license) {
    throw new HttpError(404, "LICENSE_NOT_FOUND", "License key was not found.");
  }

  if (license.status !== "active") {
    throw new HttpError(403, "LICENSE_REVOKED", "License key has been revoked.");
  }

  if (license.expires_at && Date.parse(license.expires_at) <= Date.now()) {
    throw new HttpError(403, "LICENSE_EXPIRED", "License key has expired.");
  }
}

async function requireAdmin(request, env) {
  const expectedToken = requireEnv(env, "ADMIN_API_TOKEN");
  const header = request.headers.get("Authorization") ?? "";
  const token = header.startsWith("Bearer ") ? header.slice("Bearer ".length).trim() : "";

  if (!token || !timingSafeEqual(token, expectedToken)) {
    throw new HttpError(401, "UNAUTHORIZED", "Admin token is missing or invalid.");
  }
}

async function readJson(request) {
  try {
    const body = await request.json();
    if (!body || typeof body !== "object" || Array.isArray(body)) {
      throw new HttpError(400, "INVALID_JSON", "Request body must be a JSON object.");
    }

    return body;
  } catch (error) {
    if (error instanceof HttpError) {
      throw error;
    }

    throw new HttpError(400, "INVALID_JSON", "Request body must be valid JSON.");
  }
}

function normalizePath(pathname) {
  if (pathname.length > 1 && pathname.endsWith("/")) {
    return pathname.slice(0, -1);
  }

  return pathname;
}

function normalizeLicenseKey(value) {
  const key = normalizeRequiredString(value, "license_key").toUpperCase().trim();
  if (!/^CDSK-[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/.test(key)) {
    throw new HttpError(400, "INVALID_LICENSE_KEY", "License key format is invalid.");
  }

  return key;
}

function normalizeRequiredString(value, fieldName) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new HttpError(400, "MISSING_FIELD", `${fieldName} is required.`);
  }

  return value.trim();
}

function normalizeOptionalString(value, maxLength) {
  if (value === undefined || value === null || value === "") {
    return null;
  }

  if (typeof value !== "string") {
    throw new HttpError(400, "INVALID_FIELD", "Expected a string value.");
  }

  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed.slice(0, maxLength);
}

function normalizeExpiresAt(value) {
  if (value === undefined || value === null || value === "") {
    return null;
  }

  if (typeof value !== "string") {
    throw new HttpError(400, "INVALID_EXPIRES_AT", "expires_at must be an ISO date string.");
  }

  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) {
    throw new HttpError(400, "INVALID_EXPIRES_AT", "expires_at must be an ISO date string.");
  }

  return new Date(timestamp).toISOString();
}

function getAllowedDeviceCount(_license) {
  return 1;
}

function generateLicenseKey() {
  const bytes = new Uint8Array(20);
  crypto.getRandomValues(bytes);

  const chars = Array.from(bytes, (byte) => LICENSE_KEY_ALPHABET[byte % LICENSE_KEY_ALPHABET.length]);
  const groups = [];
  for (let index = 0; index < chars.length; index += 4) {
    groups.push(chars.slice(index, index + 4).join(""));
  }

  return `${LICENSE_KEY_PREFIX}-${groups.join("-")}`;
}

async function hashLicenseKey(env, licenseKey) {
  return hmacHex(requireEnv(env, "LICENSE_SIGNING_SECRET"), `license:${licenseKey}`);
}

async function hashDeviceId(env, deviceId) {
  return hmacHex(requireEnv(env, "LICENSE_SIGNING_SECRET"), `device:${deviceId}`);
}

async function signLicenseToken(env, payload) {
  const header = base64UrlEncodeText(JSON.stringify({ alg: "HS256", typ: "JWT" }));
  const body = base64UrlEncodeText(JSON.stringify(payload));
  const signature = await hmacBase64Url(
    requireEnv(env, "LICENSE_SIGNING_SECRET"),
    `${header}.${body}`,
  );

  return `${header}.${body}.${signature}`;
}

async function verifyLicenseToken(env, token) {
  const parts = token.split(".");
  if (parts.length !== 3) {
    throw new HttpError(401, "INVALID_TOKEN", "Invalid license token.");
  }

  const [header, body, signature] = parts;
  const expectedSignature = await hmacBase64Url(
    requireEnv(env, "LICENSE_SIGNING_SECRET"),
    `${header}.${body}`,
  );

  if (!timingSafeEqual(signature, expectedSignature)) {
    throw new HttpError(401, "INVALID_TOKEN", "Invalid license token.");
  }

  try {
    const payload = JSON.parse(base64UrlDecodeText(body));
    if (payload.exp && Date.parse(payload.exp) <= Date.now()) {
      throw new HttpError(401, "TOKEN_EXPIRED", "License token has expired.");
    }

    return payload;
  } catch (error) {
    if (error instanceof HttpError) {
      throw error;
    }

    throw new HttpError(401, "INVALID_TOKEN", "Invalid license token.");
  }
}

async function hmacHex(secret, data) {
  const signature = await hmacBytes(secret, data);
  return Array.from(signature, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function hmacBase64Url(secret, data) {
  return bytesToBase64Url(await hmacBytes(secret, data));
}

async function hmacBytes(secret, data) {
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const signature = await crypto.subtle.sign("HMAC", key, encoder.encode(data));
  return new Uint8Array(signature);
}

function base64UrlEncodeText(value) {
  return bytesToBase64Url(encoder.encode(value));
}

function base64UrlDecodeText(value) {
  const base64 = value.replaceAll("-", "+").replaceAll("_", "/");
  const padded = base64.padEnd(base64.length + ((4 - (base64.length % 4)) % 4), "=");
  const binary = atob(padded);
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
  return decoder.decode(bytes);
}

function bytesToBase64Url(bytes) {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }

  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

function timingSafeEqual(left, right) {
  const leftBytes = encoder.encode(String(left ?? ""));
  const rightBytes = encoder.encode(String(right ?? ""));
  let diff = leftBytes.length ^ rightBytes.length;
  const maxLength = Math.max(leftBytes.length, rightBytes.length);

  for (let index = 0; index < maxLength; index += 1) {
    diff |= (leftBytes[index] ?? 0) ^ (rightBytes[index] ?? 0);
  }

  return diff === 0;
}

function requireEnv(env, name) {
  const value = env[name];
  if (typeof value !== "string" || value.trim() === "") {
    throw new HttpError(500, "SERVER_MISCONFIGURED", `${name} is not configured.`);
  }

  return value;
}

function json(data, status = 200) {
  return new Response(JSON.stringify(data, null, 2), {
    status,
    headers: {
      ...CORS_HEADERS,
      "Content-Type": "application/json; charset=utf-8",
    },
  });
}
