// Throwaway spike for ADR-014 (DotNet provider seam). Not wired into the runtime.
//
// Protocol: one JSON object per line on stdin, one JSON object per line on stdout.
// Values are tagged: {"t":"s","v":"text"}, {"t":"i4","v":1}, {"t":"i8","v":1},
// {"t":"b","v":true}, {"t":"d","v":"1.5"} (decimal as string), {"t":"f8","v":1.5},
// {"t":"bytes","v":"<base64>"}, {"t":"enum","type":"System.Base64FormattingOptions","v":"None"},
// {"t":"h","v":12} (object handle held by the sidecar), {"t":"null"}.
// Dispatch is reflection by name with an overload pick on the runtime types of the
// arguments, the way BC's NavAutomationHelper.GetMethodInfo does it (simplified).
using System.Globalization;
using System.Reflection;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

CultureInfo.DefaultThreadCurrentCulture = CultureInfo.InvariantCulture;
CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;

var probeDirs = new List<string>();
var handles = new Dictionary<long, object>();
long nextHandle = 1;

AssemblyLoadContext.Default.Resolving += (ctx, name) =>
{
    foreach (var dir in probeDirs)
    {
        var path = Path.Combine(dir, name.Name + ".dll");
        if (File.Exists(path)) return ctx.LoadFromAssemblyPath(path);
    }
    return null;
};

// Keep stray Console.Write calls from corrupting the protocol stream.
var protocolOut = new StreamWriter(Console.OpenStandardOutput(), new UTF8Encoding(false)) { AutoFlush = true, NewLine = "\n" };
Console.SetOut(Console.Error);
var protocolIn = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false));

string? line;
while ((line = protocolIn.ReadLine()) != null)
{
    JsonObject resp;
    long id = 0;
    try
    {
        var req = JsonNode.Parse(line)!.AsObject();
        id = req["id"]?.GetValue<long>() ?? 0;
        resp = Handle(req);
    }
    catch (Exception ex)
    {
        resp = Error(ex);
    }
    resp["id"] = id;
    protocolOut.WriteLine(resp.ToJsonString());
}

JsonObject Ok(JsonNode? value) => new() { ["ok"] = true, ["value"] = value };

JsonObject Error(Exception ex)
{
    while (ex is TargetInvocationException { InnerException: not null } tie) ex = tie.InnerException!;
    return new JsonObject
    {
        ["ok"] = false,
        ["error"] = new JsonObject { ["type"] = ex.GetType().FullName, ["message"] = ex.Message },
    };
}

JsonObject Handle(JsonObject req)
{
    switch (req["op"]!.GetValue<string>())
    {
        case "ping":
            return Ok(new JsonObject { ["runtime"] = System.Runtime.InteropServices.RuntimeInformation.FrameworkDescription });
        case "probe":
            foreach (var d in req["dirs"]!.AsArray()) probeDirs.Add(d!.GetValue<string>());
            return Ok(null);
        case "load":
        {
            var asm = AssemblyLoadContext.Default.LoadFromAssemblyPath(req["path"]!.GetValue<string>());
            return Ok(new JsonObject { ["assembly"] = asm.FullName });
        }
        case "new":
        {
            var type = ResolveType(req["type"]!.GetValue<string>());
            var obj = Invoke(type, null, ".ctor", Args(req), isCtor: true);
            return Ok(Encode(obj));
        }
        case "call":
        {
            object? target = req["target"] is JsonNode t ? handles[t.GetValue<long>()] : null;
            var type = target?.GetType() ?? ResolveType(req["type"]!.GetValue<string>());
            return Ok(Encode(Invoke(type, target, req["method"]!.GetValue<string>(), Args(req), false)));
        }
        case "get":
        case "set":
        {
            object? target = req["target"] is JsonNode t ? handles[t.GetValue<long>()] : null;
            var type = target?.GetType() ?? ResolveType(req["type"]!.GetValue<string>());
            var flags = BindingFlags.Public | (target == null ? BindingFlags.Static : BindingFlags.Instance);
            var prop = type.GetProperty(req["name"]!.GetValue<string>(), flags)
                ?? throw new MissingMemberException(type.FullName, req["name"]!.GetValue<string>());
            if (req["op"]!.GetValue<string>() == "get") return Ok(Encode(prop.GetValue(target)));
            prop.SetValue(target, Coerce(Decode(req["value"]), prop.PropertyType));
            return Ok(null);
        }
        case "release":
            handles.Remove(req["handle"]!.GetValue<long>());
            return Ok(null);
        case "count":
            return Ok(JsonValue.Create(handles.Count));
        case "inspect": // diagnostic: public static methods of a type that take and return simple values
        {
            var type = ResolveType(req["type"]!.GetValue<string>());
            var arr = new JsonArray();
            foreach (var m in type.GetMethods(BindingFlags.Public | BindingFlags.Static))
            {
                var ps = m.GetParameters();
                arr.Add($"{m.ReturnType.Name} {m.Name}({string.Join(", ", ps.Select(p => p.ParameterType.Name + " " + p.Name))})");
            }
            return Ok(arr);
        }
        case "types": // diagnostic: public types of a loaded assembly whose full name contains a filter
        {
            var filter = req["filter"]!.GetValue<string>();
            var arr = new JsonArray();
            foreach (var asm in AppDomain.CurrentDomain.GetAssemblies())
            {
                Type[] types;
                try { types = asm.GetExportedTypes(); } catch { continue; }
                foreach (var t in types)
                    if (t.FullName!.Contains(filter, StringComparison.OrdinalIgnoreCase)) arr.Add(t.FullName);
            }
            return Ok(arr);
        }
        default:
            throw new NotSupportedException("unknown op " + req["op"]);
    }
}

Type ResolveType(string name)
{
    var t = Type.GetType(name);
    if (t != null) return t;
    foreach (var asm in AppDomain.CurrentDomain.GetAssemblies())
    {
        t = asm.GetType(name);
        if (t != null) return t;
    }
    throw new TypeLoadException($"type '{name}' not found in any loaded assembly");
}

object?[] Args(JsonObject req) =>
    req["args"] is JsonArray a ? a.Select(Decode).ToArray() : Array.Empty<object?>();

object? Decode(JsonNode? n)
{
    if (n == null) return null;
    var o = n.AsObject();
    switch (o["t"]!.GetValue<string>())
    {
        case "null": return null;
        case "s": return o["v"]!.GetValue<string>();
        case "i4": return o["v"]!.GetValue<int>();
        case "i8": return o["v"]!.GetValue<long>();
        case "b": return o["v"]!.GetValue<bool>();
        case "f8": return o["v"]!.GetValue<double>();
        case "d": return decimal.Parse(o["v"]!.GetValue<string>(), CultureInfo.InvariantCulture);
        case "bytes": return Convert.FromBase64String(o["v"]!.GetValue<string>());
        case "enum": return Enum.Parse(ResolveType(o["type"]!.GetValue<string>()), o["v"]!.GetValue<string>());
        case "h": return handles[o["v"]!.GetValue<long>()];
        default: throw new NotSupportedException("unknown tag " + o["t"]);
    }
}

JsonNode Encode(object? v)
{
    switch (v)
    {
        case null: return new JsonObject { ["t"] = "null" };
        case string s: return new JsonObject { ["t"] = "s", ["v"] = s };
        case int i: return new JsonObject { ["t"] = "i4", ["v"] = i };
        case long l: return new JsonObject { ["t"] = "i8", ["v"] = l };
        case bool b: return new JsonObject { ["t"] = "b", ["v"] = b };
        case double d: return new JsonObject { ["t"] = "f8", ["v"] = d };
        case decimal m: return new JsonObject { ["t"] = "d", ["v"] = m.ToString(CultureInfo.InvariantCulture) };
        case byte[] bytes: return new JsonObject { ["t"] = "bytes", ["v"] = Convert.ToBase64String(bytes) };
        case Enum e: return new JsonObject { ["t"] = "enum", ["type"] = e.GetType().FullName, ["v"] = e.ToString() };
        default:
            var h = nextHandle++;
            handles[h] = v;
            return new JsonObject { ["t"] = "h", ["v"] = h, ["type"] = v.GetType().FullName };
    }
}

// Overload pick: same name and arity, then the candidate whose parameters accept the
// runtime types of the arguments with the fewest conversions.
object? Invoke(Type type, object? target, string name, object?[] args, bool isCtor)
{
    var flags = BindingFlags.Public | (isCtor || target != null ? BindingFlags.Instance : BindingFlags.Static);
    if (target != null) flags |= BindingFlags.Static;
    IEnumerable<MethodBase> cands = isCtor
        ? type.GetConstructors(BindingFlags.Public | BindingFlags.Instance)
        : type.GetMethods(flags).Where(m => m.Name == name);
    MethodBase? best = null;
    var bestScore = int.MaxValue;
    foreach (var c in cands)
    {
        var ps = c.GetParameters();
        if (ps.Length != args.Length) continue;
        var score = 0;
        var ok = true;
        for (var i = 0; i < ps.Length && ok; i++)
        {
            var pt = ps[i].ParameterType;
            var a = args[i];
            if (a == null) { ok = !pt.IsValueType; continue; }
            if (pt == a.GetType()) continue;
            if (pt.IsAssignableFrom(a.GetType())) { score += 1; continue; }
            if (pt.IsEnum && a is int) { score += 2; continue; }
            if (pt.IsPrimitive && a is IConvertible && a.GetType().IsPrimitive) { score += 3; continue; }
            ok = false;
        }
        if (ok && score < bestScore) { best = c; bestScore = score; }
    }
    if (best == null) throw new MissingMethodException($"{type.FullName}.{name} with {args.Length} argument(s) matching the given types");
    var ps2 = best.GetParameters();
    var coerced = args.Select((a, i) => Coerce(a, ps2[i].ParameterType)).ToArray();
    return best is ConstructorInfo ci ? ci.Invoke(coerced) : ((MethodInfo)best).Invoke(target, coerced);
}

object? Coerce(object? a, Type pt)
{
    if (a == null) return null;
    if (pt.IsEnum && a is int i) return Enum.ToObject(pt, i);
    if (pt.IsAssignableFrom(a.GetType())) return a;
    return Convert.ChangeType(a, pt, CultureInfo.InvariantCulture);
}
