/**
 * ModelScope 认证方式对比探测（仍然匿名，只用假凭据看**错误码/错误文案变化**）。
 *
 * 目的：区分「Cookie 认证」与「Bearer token 认证」两套体系。
 */
const base = "https://www.modelscope.cn";
const fakeCookie = "csrf_session=FAKE-PROBE-VALUE; csrf_token=FAKE-PROBE-VALUE";

const cases = [
  { path: "/api/v1/users/login/info", headers: {} },
  { path: "/api/v1/users/login/info", headers: { cookie: fakeCookie } },
  { path: "/openapi/v1/magicubes/balance", headers: {} },
  { path: "/openapi/v1/magicubes/balance", headers: { cookie: fakeCookie } },
  { path: "/openapi/v1/magicubes/balance", headers: { authorization: "Bearer FAKE-PROBE-TOKEN" } },
  { path: "/openapi/v1/magicubes/transactions", headers: { cookie: fakeCookie } },
  { path: "/openapi/v1/magicubes/transactions", headers: { authorization: "Bearer FAKE-PROBE-TOKEN" } },
  { path: "/openapi/v1/users/me", headers: { cookie: fakeCookie } },
];

for (const c of cases) {
  const label = Object.keys(c.headers).length
    ? Object.keys(c.headers).map((k) => k).join("+")
    : "无凭据";
  try {
    const res = await fetch(base + c.path, {
      headers: {
        accept: "application/json",
        "user-agent": "Mozilla/5.0",
        ...c.headers,
      },
      redirect: "manual",
    });
    const body = (await res.text()).replace(/\s+/g, " ").slice(0, 190);
    console.log(`${res.status} [${label}] ${c.path}\n    ${body}\n`);
  } catch (e) {
    console.log(`ERR [${label}] ${c.path}  ${String(e).split("\n")[0]}\n`);
  }
}
