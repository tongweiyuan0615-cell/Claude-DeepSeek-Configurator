# Claude DeepSeek License Server

这是 Cloudflare Workers + D1 授权服务，用来替代桌面端写死激活码的方案。

当前授权规则：

- 一个激活码最多绑定 1 台设备。
- 默认永久有效。
- 管理员可以吊销激活码。
- 用户换电脑时，管理员手动重置设备绑定。
- 暂时不做年费、自助重置、多设备授权。

## 目录

```text
license-server/
├── src/index.js              # Worker API
├── schema.sql                # D1 数据库结构
├── migrations/0001_initial.sql
├── wrangler.toml.example     # Wrangler 配置模板
├── package.json
└── README.md
```

## 安全设计

- 原始激活码不保存到 D1，只保存 `LICENSE_SIGNING_SECRET` 计算出的 HMAC 哈希。
- 设备 ID 不保存明文，只保存 HMAC 哈希。
- 管理接口必须带 `Authorization: Bearer <ADMIN_API_TOKEN>`。
- `LICENSE_SIGNING_SECRET` 和 `ADMIN_API_TOKEN` 必须通过 Cloudflare Secret 配置，不提交到 Git。
- 这个服务只处理激活码和设备绑定，不接收、不保存 DeepSeek API Key。

## API

### Public

`GET /health`

服务健康检查。

`POST /activate`

桌面端首次激活或重新激活当前设备。

```json
{
  "license_key": "CDSK-XXXX-XXXX-XXXX-XXXX-XXXX",
  "device_id": "stable-device-id",
  "platform": "windows",
  "app_version": "0.2.0"
}
```

成功后返回 `license_token`，桌面端后续可以存到本机。

`POST /check`

桌面端启动时校验本机授权状态。

```json
{
  "license_token": "returned-token",
  "device_id": "stable-device-id",
  "platform": "windows",
  "app_version": "0.2.0"
}
```

### Admin

所有 Admin 接口都需要：

```http
Authorization: Bearer <ADMIN_API_TOKEN>
```

`POST /admin/licenses`

生成新激活码。返回的 `license_key` 只出现一次，请保存到你的销售记录里。

```json
{
  "note": "customer email or order id"
}
```

`GET /admin/licenses?limit=50`

列出授权记录，不返回原始激活码和哈希。

`POST /admin/licenses/revoke`

吊销激活码。

```json
{
  "license_key": "CDSK-XXXX-XXXX-XXXX-XXXX-XXXX"
}
```

也可以用：

```json
{
  "license_id": "license uuid"
}
```

`POST /admin/licenses/reset-device`

清空设备绑定。用户换电脑时，用这个接口让同一个激活码可以在新设备重新激活。

```json
{
  "license_key": "CDSK-XXXX-XXXX-XXXX-XXXX-XXXX"
}
```

## 部署步骤

### 1. 安装依赖

```powershell
cd license-server
npm install
```

### 2. 准备 Wrangler 配置

复制模板：

```powershell
Copy-Item wrangler.toml.example wrangler.toml
```

`wrangler.toml` 已被 `.gitignore` 忽略，可以安全填入你自己的 D1 database id。

### 3. 创建 D1 数据库

可以在 Cloudflare 控制台创建，也可以用 Wrangler：

```powershell
npx wrangler d1 create claude_deepseek_license
```

把输出里的 `database_id` 填到 `wrangler.toml`：

```toml
[[d1_databases]]
binding = "DB"
database_name = "claude_deepseek_license"
database_id = "你的 database_id"
```

### 4. 配置 Secret

不要把 Secret 写进代码或 Git。

```powershell
npx wrangler secret put LICENSE_SIGNING_SECRET
npx wrangler secret put ADMIN_API_TOKEN
```

`LICENSE_SIGNING_SECRET` 使用你已经本地生成并保存的值。

`ADMIN_API_TOKEN` 建议另行生成一个强随机值，例如：

```powershell
node -e "console.log(crypto.randomBytes(32).toString('base64url'))"
```

### 5. 初始化 D1 表

```powershell
npm run d1:apply:remote
```

### 6. 部署 Worker

```powershell
npm run deploy
```

部署目标服务名：

```text
claude-deepseek-license
```

当前 Worker 地址：

```text
https://claude-deepseek-license.tongweiyuan0615.workers.dev
```

## 管理员常用命令

下面示例用 PowerShell。先设置：

```powershell
$base = "https://claude-deepseek-license.tongweiyuan0615.workers.dev"
$admin = "你的 ADMIN_API_TOKEN"
```

健康检查：

```powershell
curl.exe "$base/health"
```

生成激活码：

```powershell
curl.exe -X POST "$base/admin/licenses" `
  -H "Authorization: Bearer $admin" `
  -H "Content-Type: application/json" `
  -d "{\"note\":\"order-001\"}"
```

吊销激活码：

```powershell
curl.exe -X POST "$base/admin/licenses/revoke" `
  -H "Authorization: Bearer $admin" `
  -H "Content-Type: application/json" `
  -d "{\"license_key\":\"CDSK-XXXX-XXXX-XXXX-XXXX-XXXX\"}"
```

重置设备绑定：

```powershell
curl.exe -X POST "$base/admin/licenses/reset-device" `
  -H "Authorization: Bearer $admin" `
  -H "Content-Type: application/json" `
  -d "{\"license_key\":\"CDSK-XXXX-XXXX-XXXX-XXXX-XXXX\"}"
```

## 本地开发

本地开发时可创建 `license-server/.dev.vars`：

```text
LICENSE_SIGNING_SECRET=local-dev-secret
ADMIN_API_TOKEN=local-admin-token
```

`.dev.vars` 已被 `.gitignore` 忽略。

初始化本地 D1：

```powershell
npm run d1:apply:local
npm run dev
```

## 下一步接入桌面端

这个提交只新增服务端，不修改 Windows/macOS 客户端。下一步需要在 Tauri 客户端里做：

1. 生成稳定的本机 `device_id`。
2. 用户输入激活码时调用 `/activate`。
3. 把返回的 `license_token` 保存到本机。
4. App 启动或部署前调用 `/check`。
5. 移除当前写死激活码逻辑。
