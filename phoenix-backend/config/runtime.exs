import Config

if System.get_env("PHX_SERVER") do
  config :complete, CompleteWeb.Endpoint, server: true
end

config :complete, CompleteWeb.Endpoint,
  http: [port: String.to_integer(System.get_env("PORT", "4000"))]

if token = System.get_env("AUTH_TOKEN") do
  config :complete, :auth_token, token
end

# Phase 3: real upstream inference. Unset INFERENCE_BASE_URL → MockProvider.
if base_url = System.get_env("INFERENCE_BASE_URL") do
  config :complete, :inference,
    base_url: base_url,
    model: System.get_env("INFERENCE_MODEL", "Qwen/Qwen2.5-Coder-32B-Instruct"),
    fim_template:
      System.get_env(
        "INFERENCE_FIM_TEMPLATE",
        "<|fim_prefix|>{prefix}<|fim_suffix|>{suffix}<|fim_middle|>"
      ),
    max_tokens: String.to_integer(System.get_env("INFERENCE_MAX_TOKENS", "512")),
    temperature: String.to_float(System.get_env("INFERENCE_TEMPERATURE", "0.0")),
    timeout_ms: String.to_integer(System.get_env("INFERENCE_TIMEOUT_MS", "25000"))
end

if config_env() == :prod do
  unless System.get_env("AUTH_TOKEN") do
    raise "environment variable AUTH_TOKEN is missing"
  end

  secret_key_base =
    System.get_env("SECRET_KEY_BASE") ||
      raise """
      environment variable SECRET_KEY_BASE is missing.
      You can generate one by calling: mix phx.gen.secret
      """

  host = System.get_env("PHX_HOST") || "example.com"

  config :complete, CompleteWeb.Endpoint,
    url: [host: host, port: 443, scheme: "https"],
    http: [ip: {0, 0, 0, 0, 0, 0, 0, 0}],
    secret_key_base: secret_key_base
end
