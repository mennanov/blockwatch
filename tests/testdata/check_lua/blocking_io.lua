function validate(ctx, content)
  -- In sandboxed mode `os` is absent, so there is nothing to block on: pass, and stay inert during
  -- whole-tree scans that run in the default mode. The blocking path below is reached only when the
  -- run makes `os` available (safe/unsafe mode).
  if os == nil then
    return nil
  end

  os.execute("exec sleep 10 >/dev/null 2>&1 </dev/null")
  return nil
end
