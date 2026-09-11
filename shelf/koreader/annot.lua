-- annot.lua —— 读一本书的 KOReader 标注 sidecar（<book>.sdr/metadata.<ext>.lua），
-- 跟 merge.lua 同款"设备端用 KOReader 自带 luajit 跑"套路，见 shelf/services/koreader-serve/src/annot.rs。
-- 用法: luajit annot.lua <metadata.*.lua 路径>
-- 输出: stdout 一行 JSON {"title":<string|null>,"annotations":[{...KOReader 原始字段...}]}
--   sidecar 不存在/解析失败都当"这本书还没有标注"，输出 {"title":null,"annotations":[]}，退出码仍是 0——
--   调用方（annot.rs）批量扫全部书，个别书解析失败不该整批失败。
local path = arg[1]
if not path then
  io.stderr:write("用法: annot.lua <metadata.*.lua 路径>\n"); os.exit(2)
end

local function empty()
  io.write('{"title":null,"annotations":[]}\n')
  os.exit(0)
end

local f = io.open(path, "r")
if not f then empty() end
local src = f:read("*a"); f:close()
local chunk = loadstring(src, "=" .. path)
if not chunk then empty() end
local ok, t = pcall(chunk)
if not ok or type(t) ~= "table" then empty() end

-- JSON 序列化：跟 merge.lua 的 jval 同一套写法，独立成文件（两个脚本各自单独被 luajit 起一个新进程跑，
-- 没有"引入模块"这回事，重复这一小段比硬凑一个共享 require 路径更简单）。多一层数组识别（annotations
-- 是数组，merge.lua 只需要处理对象）。
local function jstr(s) return '"' .. s:gsub('[%c"\\]', function(c) return string.format("\\u%04x", c:byte()) end) .. '"' end
local function jval(v)
  local ty = type(v)
  if ty == "nil" then return "null" end
  if ty == "boolean" then return tostring(v) end
  if ty == "number" then return (v % 1 == 0) and string.format("%d", v) or tostring(v) end
  if ty == "string" then return jstr(v) end
  if ty == "table" then
    local n = 0
    for _ in pairs(v) do n = n + 1 end
    if n == 0 then return "[]" end
    local is_array = true
    for i = 1, n do
      if v[i] == nil then is_array = false; break end
    end
    if is_array then
      local parts = {}
      for i = 1, n do parts[#parts + 1] = jval(v[i]) end
      return "[" .. table.concat(parts, ",") .. "]"
    end
    local keys = {}
    for k in pairs(v) do keys[#keys + 1] = k end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    local parts = {}
    for _, k in ipairs(keys) do parts[#parts + 1] = jstr(tostring(k)) .. ":" .. jval(v[k]) end
    return "{" .. table.concat(parts, ",") .. "}"
  end
  return jstr(tostring(v))
end

local props = t.doc_props
local title = (type(props) == "table" and type(props.title) == "string" and props.title ~= "") and props.title or nil
io.write('{"title":' .. jval(title) .. ',"annotations":' .. jval(t.annotations or {}) .. "}\n")
