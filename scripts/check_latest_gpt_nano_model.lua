-- Checks that the configured model is the latest nano model from OpenAI.
-- Reads the model list from models.dev, because it needs no API key.
-- Requires: BLOCKWATCH_LUA_MODE=safe, jq, curl.

local MODELS_URL = "https://models.dev/api.json"

local function check_dependencies()
    if not io then
        return "io Lua module is unavailable"
    end
    return nil
end

local function fetch_latest_nano_model()
    -- Only base aliases such as "gpt-5-nano" or "gpt-5.4-nano" match. So a dated snapshot
    -- or another model whose ID contains "nano" can't be picked.
    -- `release_date` is always YYYY-MM-DD, so comparing it as a string sorts it by date.
    local cmd = string.format(
        'curl -sS "%s"'
        .. ' | jq -r \'.openai.models | to_entries'
        .. ' | map(select(.key | test("^gpt-[0-9.]+-nano$")))'
        .. ' | max_by(.value.release_date) | .key // empty\'',
        MODELS_URL
    )
    local handle = io.popen(cmd)
    if not handle then
        return nil, "failed to run curl | jq"
    end
    local output = handle:read("*a")
    handle:close()

    local model = output and output:match("^%s*(.-)%s*$") or ""
    if model == "" then
        return nil, "no base nano model (gpt-N-nano) found at " .. MODELS_URL
    end

    return model, nil
end


function validate(ctx, content)
    local dep_err = check_dependencies()
    if dep_err then
        return dep_err
    end

    -- The block's `check-lua-pattern` selects the model name, so `content` is an array of matches.
    local configured_model = content[1]
    if not configured_model then
        return "check-lua-pattern matched no model name in the block"
    end

    local latest_model, fetch_err = fetch_latest_nano_model()
    if fetch_err then
        return fetch_err
    end

    if latest_model == configured_model then
        return nil
    end

    return string.format(
        "expected %q but the latest nano model is %q",
        configured_model, latest_model
    )
end
