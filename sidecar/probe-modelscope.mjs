/**
 * ModelScope 接口面探测（匿名态）。
 *
 * 判据：**存在的路径**回统一错误体（`InvalidAuthentication` / `禁止访问`），
 * **不存在的路径**回 404 或 HTML。两者一眼能分。
 */
const base = "https://www.modelscope.cn";
const paths = [
  "/api/v1/users/login/info",
  "/openapi/v1/users/login/info",
  "/openapi/v1/magicubes/transactions",
  "/openapi/v1/magicubes/balance",
  "/openapi/v1/magicubes",
  "/openapi/v1/magicubes/total",
  "/api/v1/magicubes/transactions",
  "/api/v1/users/me",
  "/api/v1/users/binding",
  "/openapi/v1/users/me",
];

for (const path of paths) {
  try {
    const res = await fetch(base + path, {
      headers: { accept: "application/json", "user-agent": "Mozilla/5.0" },
      redirect: "manual",
    });
    const body = (await res.text()).replace(/\s+/g, " ").slice(0, 170);
    console.log(`${res.status} ${path}\n    ${body}\n`);
  } catch (e) {
    console.log(`ERR ${path}  ${String(e).split("\n")[0]}\n`);
  }
}
