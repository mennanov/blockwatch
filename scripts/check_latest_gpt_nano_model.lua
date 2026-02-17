-- Checks that the configured model is the latest nano model from OpenAI.
-- Requires: BLOCKWATCH_LUA_MODE=safe, BLOCKWATCH_AI_API_KEY, jq, curl.

local function check_dependencies()
    if not os or not io then
        return "os or io Lua module is unavailable"
    end
    local api_key = os.getenv("BLOCKWATCH_AI_API_KEY")
    if not api_key or api_key == "" then
        return "BLOCKWATCH_AI_API_KEY is not set"
    end
    return nil
end

local function fetch_nano_models(api_key, api_url)
    api_url = api_url or "https://api.openai.com/v1"
    api_url = api_url:gsub("/$", "")

    -- Fetch nano model IDs via jq, one per line
    local cmd = string.format(
        'curl -sS -H "Authorization: Bearer %s" "%s/models"'
        .. ' | jq -r \'.data[].id | select(test("nano"))\'',
        api_key, api_url
    )
    local handle = io.popen(cmd)
    if not handle then
        return nil, "failed to run curl | jq"
    end
    local output = handle:read("*a")
    handle:close()

    if not output or output == "" then
        return nil, "no nano models found in OpenAI API response"
    end

    return output, nil
end

local function find_best_nano_model(output)
    -- Find the latest-version base alias (e.g. "gpt-6-nano" over "gpt-5-nano").
    -- Dated variants like "gpt-5-nano-2025-08-07" are skipped.
    local best_name = nil
    local best_version = -1
    for line in output:gmatch("[^\n]+") do
        local ver = line:match("^gpt%-(%d+)%-nano$")
        if ver and tonumber(ver) > best_version then
            best_version = tonumber(ver)
            best_name = line
        end
    end
    return best_name
end


function validate(ctx, content)
    local dep_err = check_dependencies()
    if dep_err then
        return dep_err
    end

    local api_key = os.getenv("BLOCKWATCH_AI_API_KEY")
    local api_url = os.getenv("BLOCKWATCH_AI_API_URL")

    local output, fetch_err = fetch_nano_models(api_key, api_url)
    if fetch_err then
        return fetch_err
    end

    local best_name = find_best_nano_model(output)
    if not best_name then
        return "no base nano model (gpt-N-nano) found in API response"
    end

    if best_name == content then
        return nil
    end

    return string.format(
        "expected %q but the latest nano model is %q",
        content, best_name
    )
end
