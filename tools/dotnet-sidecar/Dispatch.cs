// Requests and reflection dispatch (ADR-014 sections 4, 7 and 10).
using System.Globalization;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text.Json.Nodes;

namespace AlDotNetSidecar;

public sealed class Server
{
    readonly Resolver resolver = new();
    readonly Marshaller marshal;

    public Server()
    {
        marshal = new Marshaller(resolver.ResolveLoaded);
    }

    public JsonObject Handle(JsonObject req)
    {
        marshal.BeginRequest();
        switch (req["op"]!.GetValue<string>())
        {
            case "hello":
            {
                int theirs = req["protocol"]?.GetValue<int>() ?? 0;
                return Ok(new JsonObject
                {
                    ["protocol"] = Program.ProtocolVersion,
                    ["compatible"] = theirs == Program.ProtocolVersion,
                    ["runtime"] = RuntimeInformation.FrameworkDescription,
                    ["major"] = Environment.Version.Major,
                    ["culture"] = CultureInfo.CurrentCulture.Name,
                    ["timezone"] = TimeZoneInfo.Local.Id,
                    ["cwd"] = Directory.GetCurrentDirectory(),
                });
            }
            case "probe":
                resolver.AddDirs(req["dirs"]!.AsArray().Select(d =>
                    new Resolver.Dir(d!["path"]!.GetValue<string>(), d["recursive"]?.GetValue<bool>() ?? false)));
                return Ok(null);
            case "release":
                marshal.Release(req["handles"]!.AsArray());
                return Ok(null);
            case "count":
                return Ok(marshal.HandleCount);
            case "bye":
                return Ok(null);
            case "construct":
            {
                var type = DeclaredType(req["type"]!);
                var args = DecodeArgs(req);
                var (best, coerced, _) = Pick(type.GetConstructors(BindingFlags.Public | BindingFlags.Instance), args, $"new {type.FullName}");
                object? obj = ((ConstructorInfo)best).Invoke(coerced);
                return Done(obj, best, coerced, args);
            }
            case "invoke":
            {
                var (target, type) = Target(req["target"]!);
                string name = req["member"]!.GetValue<string>();
                var args = DecodeArgs(req);
                var flags = BindingFlags.Public | BindingFlags.Static | (target != null ? BindingFlags.Instance : 0);
                var cands = type.GetMethods(flags).Where(m => m.Name == name && !m.IsGenericMethodDefinition).ToArray();
                if (cands.Length == 0)
                    throw new Refusal(target == null && type.GetMethods(BindingFlags.Public | BindingFlags.Instance).Any(m => m.Name == name)
                        ? $"'{type.FullName}.{name}' is an instance member and the variable holds no object"
                        : $"member '{name}' not found on '{type.FullName}'");
                var (best, coerced, _) = Pick(cands, args, $"{type.FullName}.{name}");
                object? result = ((MethodInfo)best).Invoke(((MethodInfo)best).IsStatic ? null : target, coerced);
                return Done(result, best, coerced, args);
            }
            case "get":
            {
                var (target, type) = Target(req["target"]!);
                string name = req["member"]!.GetValue<string>();
                var flags = BindingFlags.Public | BindingFlags.Static | (target != null ? BindingFlags.Instance : 0);
                var prop = type.GetProperty(name, flags, null, null, Type.EmptyTypes, null);
                if (prop != null && prop.GetMethod != null)
                    return Done(prop.GetValue(prop.GetMethod.IsStatic ? null : target), null, null, null);
                var field = type.GetField(name, flags);
                if (field != null)
                    return Done(field.GetValue(field.IsStatic ? null : target), null, null, null);
                // AL lets a method be called without parentheses.
                var m = type.GetMethods(flags).Where(x => x.Name == name && x.GetParameters().Length == 0 && !x.IsGenericMethodDefinition).FirstOrDefault();
                if (m != null)
                    return Done(m.Invoke(m.IsStatic ? null : target, Array.Empty<object>()), null, null, null);
                throw new Refusal($"property, field or method '{name}' not found on '{type.FullName}'");
            }
            case "set":
            {
                var (target, type) = Target(req["target"]!);
                string name = req["member"]!.GetValue<string>();
                var value = marshal.Decode(req["value"], 0);
                var flags = BindingFlags.Public | BindingFlags.Static | (target != null ? BindingFlags.Instance : 0);
                var prop = type.GetProperty(name, flags, null, null, Type.EmptyTypes, null);
                if (prop != null && prop.SetMethod != null)
                {
                    var v = Coerce(value, prop.PropertyType) ?? throw new Refusal($"cannot assign {Describe(value)} to {type.FullName}.{name} ({prop.PropertyType.FullName})");
                    prop.SetValue(prop.SetMethod.IsStatic ? null : target, v.Value);
                    return Done(null, null, null, null);
                }
                var field = type.GetField(name, flags);
                if (field != null && !field.IsInitOnly)
                {
                    var v = Coerce(value, field.FieldType) ?? throw new Refusal($"cannot assign {Describe(value)} to {type.FullName}.{name} ({field.FieldType.FullName})");
                    field.SetValue(field.IsStatic ? null : target, v.Value);
                    return Done(null, null, null, null);
                }
                throw new Refusal($"settable property or field '{name}' not found on '{type.FullName}'");
            }
            default:
                throw new Refusal("unknown op " + req["op"]);
        }
    }

    // ---- replies ----

    static JsonObject Ok(JsonNode? value) => new() { ["ok"] = true, ["value"] = value };

    JsonObject Done(object? result, MethodBase? method, object?[]? coerced, object?[]? args)
    {
        var reply = Ok(marshal.Encode(result));
        // New values of by-reference parameters, by argument position.
        if (method != null && coerced != null)
        {
            var byref = new JsonArray();
            var ps = method.GetParameters();
            for (int i = 0; i < ps.Length; i++)
                if (ps[i].ParameterType.IsByRef && byRefFlags[i])
                    byref.Add(new JsonArray(i, marshal.Encode(coerced[i])));
            if (byref.Count > 0) reply["byref"] = byref;
        }
        var streams = new JsonObject();
        var consumed = new JsonArray();
        foreach (var s in marshal.Ins) consumed.Add(new JsonArray(s.Index, (int)s.Stream.Position));
        if (consumed.Count > 0) streams["consumed"] = consumed;
        var bridges = new JsonArray();
        foreach (var (idx, id) in marshal.NewBridges) bridges.Add(new JsonArray(idx, id));
        if (bridges.Count > 0) streams["bridges"] = bridges;
        var written = marshal.DrainBridges();
        if (written.Count > 0) streams["written"] = written;
        if (streams.Count > 0) reply["streams"] = streams;
        return reply;
    }

    /// Reflection wraps an exception raised by the called code in TargetInvocationException;
    /// anything else (type or member not found, no overload, a value that does not decode) is
    /// this process refusing the request, not an exception of the code under test.
    public static JsonObject Failure(Exception ex)
    {
        if (ex is not TargetInvocationException { InnerException: not null } tie)
            return new JsonObject { ["ok"] = false, ["kind"] = "refused", ["reason"] = ex.Message };
        ex = tie.InnerException!;
        var inner = new JsonArray();
        for (var e = ex.InnerException; e != null; e = e.InnerException)
            inner.Add(new JsonArray(e.GetType().FullName, e.Message));
        return new JsonObject
        {
            ["ok"] = false,
            ["kind"] = "exception",
            ["type"] = ex.GetType().FullName,
            ["message"] = ex.Message,
            ["inner"] = inner,
        };
    }

    // ---- targets, arguments, overloads ----

    Type DeclaredType(JsonNode t)
    {
        var o = t.AsObject();
        return resolver.ResolveType(
            o["assembly"]!.GetValue<string>(),
            o["version"]?.GetValue<string>(),
            o["culture"]?.GetValue<string>(),
            o["token"]?.GetValue<string>(),
            o["name"]!.GetValue<string>());
    }

    (object? Target, Type Type) Target(JsonNode t)
    {
        var o = t.AsObject();
        if (o["handle"] is JsonNode h)
        {
            var obj = marshal.Handle(h.GetValue<long>());
            return (obj, obj.GetType());
        }
        return (null, DeclaredType(o["static"]!));
    }

    bool[] byRefFlags = Array.Empty<bool>();

    object?[] DecodeArgs(JsonObject req)
    {
        var arr = req["args"]?.AsArray() ?? new JsonArray();
        byRefFlags = new bool[arr.Count];
        var res = new object?[arr.Count];
        for (int i = 0; i < arr.Count; i++)
        {
            byRefFlags[i] = arr[i]!["ref"]?.GetValue<bool>() ?? false;
            res[i] = marshal.Decode(arr[i]!["v"], i);
        }
        return res;
    }

    /// The candidate whose parameters accept the runtime types of the arguments with the fewest
    /// conversions: exact type, assignable, enum from integer, numeric conversion. By-reference
    /// parameters need an argument the AL caller passed as a variable.
    (MethodBase Best, object?[] Coerced, int Score) Pick(IEnumerable<MethodBase> cands, object?[] args, string what)
    {
        MethodBase? best = null;
        var bestScore = int.MaxValue;
        foreach (var c in cands)
        {
            var ps = c.GetParameters();
            if (ps.Length != args.Length) continue;
            int score = 0;
            bool ok = true;
            for (int i = 0; i < ps.Length && ok; i++)
            {
                var pt = ps[i].ParameterType;
                if (pt.IsByRef)
                {
                    if (!byRefFlags[i]) { ok = false; break; }
                    pt = pt.GetElementType()!;
                    score += 1;
                }
                var s = Cost(args[i], pt);
                if (s < 0) ok = false; else score += s;
            }
            if (ok && score < bestScore) { best = c; bestScore = score; }
        }
        if (best == null)
            throw new Refusal($"{what} has no overload taking {args.Length} argument(s) of the given types ({string.Join(", ", args.Select(Describe))})");
        var ps2 = best.GetParameters();
        var coerced = new object?[args.Length];
        for (int i = 0; i < args.Length; i++)
        {
            var pt = ps2[i].ParameterType;
            if (pt.IsByRef) pt = pt.GetElementType()!;
            coerced[i] = Coerce(args[i], pt)?.Value;
        }
        return (best, coerced, bestScore);
    }

    static string Describe(object? a) => a == null ? "null" : a.GetType().Name;

    /// -1: does not fit.
    static int Cost(object? a, Type pt)
    {
        if (a == null) return pt.IsValueType && Nullable.GetUnderlyingType(pt) == null ? -1 : 0;
        var at = a.GetType();
        if (pt == at) return 0;
        if (pt.IsAssignableFrom(at)) return 1;
        if (pt.IsEnum && (a is int or long)) return 2;
        if (pt.IsPrimitive && at.IsPrimitive && a is IConvertible && Widens(at, pt)) return 3;
        if (pt == typeof(decimal) && (a is int or long or byte or short)) return 3;
        // An AL Decimal is a .NET decimal; a double parameter takes it by numeric conversion.
        if (a is decimal && pt.IsPrimitive && pt != typeof(bool) && pt != typeof(char)) return 4;
        return -1;
    }

    static bool Widens(Type from, Type to)
    {
        if (to == typeof(bool) || from == typeof(bool)) return false;
        if (to == typeof(char) || from == typeof(char)) return false;
        return true;
    }

    sealed record Boxed(object? Value);

    static Boxed? Coerce(object? a, Type pt)
    {
        if (a == null) return pt.IsValueType && Nullable.GetUnderlyingType(pt) == null ? null : new Boxed(null);
        if (pt.IsAssignableFrom(a.GetType())) return new Boxed(a);
        if (pt.IsEnum && a is int i) return new Boxed(Enum.ToObject(pt, i));
        if (pt.IsEnum && a is long l) return new Boxed(Enum.ToObject(pt, l));
        try { return new Boxed(Convert.ChangeType(a, pt, CultureInfo.InvariantCulture)); }
        catch (Exception ex) when (ex is InvalidCastException or OverflowException or FormatException) { return null; }
    }
}
