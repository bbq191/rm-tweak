-- merge.lua —— KOReader 配置文件深合并（设备端用 KOReader 自带 luajit 跑；Lua 5.1 语法）。
-- 用法: luajit merge.lua <目标.lua> <补丁.lua> [--dry-run]
--   目标/补丁都是 `return { ... }` 形式（settings.reader.lua / defaults.custom.lua / gestures.lua 皆如此）。
--   语义：补丁里的标量覆盖；两边都是表则递归合并；补丁里值为字符串 "__DELETE__" 则删键。
--   输出：stdout 一行 JSON {"changes":[{"path":"a.b","old":…,"new":…}],"written":true|false}
--   --dry-run 只算差异不写。写入=先写 .tmp 再 rename；文件头保留 KOReader 惯例注释。
local target_path, patch_path, flag = arg[1], arg[2], arg[3]
if not target_path or not patch_path then
  io.stderr:write("用法: merge.lua <目标.lua> <补丁.lua> [--dry-run]\n"); os.exit(2)
end
local dry = (flag == "--dry-run")
local DELETE = "__DELETE__"

local function load_table(path, must)
  local f = io.open(path, "r")
  if not f then
    if must then io.stderr:write("读不到 " .. path .. "\n"); os.exit(2) end
    return {}
  end
  local src = f:read("*a"); f:close()
  local chunk, err = loadstring(src, "=" .. path)
  if not chunk then io.stderr:write("解析失败 " .. path .. ": " .. tostring(err) .. "\n"); os.exit(3) end
  local ok, t = pcall(chunk)
  if not ok or type(t) ~= "table" then io.stderr:write(path .. " 不是 return {…}\n"); os.exit(3) end
  return t
end

-- JSON 序列化（只为回执，够用即可）
local function jstr(s) return '"' .. s:gsub('[%c"\\]', function(c) return string.format("\\u%04x", c:byte()) end) .. '"' end
local function jval(v)
  local t = type(v)
  if t == "nil" then return "null" end
  if t == "boolean" then return tostring(v) end
  if t == "number" then return (v % 1 == 0) and string.format("%d", v) or tostring(v) end
  if t == "string" then return jstr(v) end
  if t == "table" then
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    local parts = {}
    for _, k in ipairs(keys) do parts[#parts + 1] = jstr(tostring(k)) .. ":" .. jval(v[k]) end
    return "{" .. table.concat(parts, ",") .. "}"
  end
  return jstr(tostring(v))
end

local changes = {}
local function merge(dst, src, path)
  for k, v in pairs(src) do
    local p = (path == "" and tostring(k)) or (path .. "." .. tostring(k))
    if v == DELETE then
      if dst[k] ~= nil then changes[#changes + 1] = { path = p, old = dst[k], new = nil }; dst[k] = nil end
    elseif type(v) == "table" and type(dst[k]) == "table" then
      merge(dst[k], v, p)
    else
      local same = (dst[k] == v)
      if type(v) == "table" and type(dst[k]) == "table" then same = false end
      if not same then
        if type(v) == "table" or type(dst[k]) == "table" then
          -- 表 vs 非表：整体替换（深拷贝）
          changes[#changes + 1] = { path = p, old = dst[k], new = v }
        else
          changes[#changes + 1] = { path = p, old = dst[k], new = v }
        end
        dst[k] = v
      end
    end
  end
end

-- Lua 序列化（KOReader 风格：键排序、字符串 %q、缩进 4 空格）
local function lkey(k)
  if type(k) == "string" and k:match("^[%a_][%w_]*$") then return k end
  if type(k) == "string" then return "[" .. string.format("%q", k) .. "]" end
  return "[" .. tostring(k) .. "]"
end
local function lser(v, indent)
  local t = type(v)
  if t == "string" then return string.format("%q", v) end
  if t == "number" then return (v % 1 == 0) and string.format("%d", v) or string.format("%.17g", v) end
  if t == "boolean" then return tostring(v) end
  if t == "table" then
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b)
      local ta, tb = type(a), type(b)
      if ta ~= tb then return ta < tb end
      return a < b
    end)
    if #keys == 0 then return "{}" end
    local pad = string.rep("    ", indent + 1)
    local out = { "{" }
    for _, k in ipairs(keys) do
      out[#out + 1] = pad .. lkey(k) .. " = " .. lser(v[k], indent + 1) .. ","
    end
    out[#out + 1] = string.rep("    ", indent) .. "}"
    return table.concat(out, "\n")
  end
  return "nil"
end

local target = load_table(target_path, false)
local patch = load_table(patch_path, true)
merge(target, patch, "")

local written = false
if not dry and #changes > 0 then
  local tmp = target_path .. ".tmp"
  local f = assert(io.open(tmp, "w"))
  f:write("-- we can read Lua syntax here!\nreturn ", lser(target, 0), "\n")
  f:close()
  assert(os.rename(tmp, target_path))
  written = true
end

local parts = {}
for _, c in ipairs(changes) do
  parts[#parts + 1] = '{"path":' .. jstr(c.path) .. ',"old":' .. jval(c.old) .. ',"new":' .. jval(c.new) .. "}"
end
io.write('{"changes":[' .. table.concat(parts, ",") .. '],"written":' .. tostring(written) .. "}\n")
