import Config

if System.get_env("PHX_SERVER") do
  config :complete, CompleteWeb.Endpoint, server: true
end

config :complete, CompleteWeb.Endpoint,
  http: [port: String.to_integer(System.get_env("PORT", "4000"))]

if token = System.get_env("AUTH_TOKEN") do
  config :complete, :auth_token, token
end
