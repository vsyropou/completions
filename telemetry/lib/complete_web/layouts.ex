defmodule CompleteWeb.Layouts do
  @moduledoc "Root layout for LiveView pages."
  use Phoenix.Component

  def root(assigns) do
    ~H"""
    <!DOCTYPE html>
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta name="csrf-token" content={Phoenix.Controller.get_csrf_token()} />
        <title>Completion Telemetry</title>
        <style>
          body { font-family: -apple-system, system-ui, sans-serif; margin: 2rem; color: #222; }
          table { border-collapse: collapse; width: 100%; font-size: 0.85rem; margin-top: 1rem; }
          th, td { text-align: left; padding: 0.4rem 0.6rem; border-bottom: 1px solid #e5e5e5; vertical-align: top; }
          th { background: #fafafa; font-weight: 600; }
          form.filter { display: flex; gap: 0.5rem; align-items: center; margin-bottom: 1rem; flex-wrap: wrap; }
          form.filter input[type="text"] { padding: 0.3rem 0.5rem; min-width: 18rem; }
          form.filter select, form.filter button { padding: 0.3rem 0.5rem; }
          .tag { display: inline-block; padding: 0.1rem 0.45rem; border-radius: 0.4rem; font-size: 0.75rem; font-weight: 600; }
          .tag-requested { background: #ececec; color: #555; }
          .tag-served { background: #dbe9fb; color: #1a4d8f; }
          .tag-accepted { background: #d8f3dc; color: #1b5e20; }
          .tag-cancelled, .tag-superseded { background: #fff3cd; color: #856404; }
          .tag-error { background: #f8d7da; color: #842029; }
          details summary { cursor: pointer; color: #0366d6; font-size: 0.8rem; }
          details pre { background: #f6f8fa; padding: 0.5rem; font-size: 0.75rem; overflow-x: auto; max-width: 60rem; }
          .cards { display: flex; gap: 1rem; margin-bottom: 1.5rem; }
          .card { border: 1px solid #e5e5e5; border-radius: 0.5rem; padding: 1rem 1.25rem; flex: 1; min-width: 12rem; }
          .card .label { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.05em; color: #666; }
          .card .big { font-size: 2rem; font-weight: 700; margin-top: 0.25rem; }
          .bar { height: 0.5rem; background: #ececec; border-radius: 0.25rem; margin-top: 0.75rem; overflow: hidden; }
          .bar .fill { height: 100%; background: #2da44e; }
          .rid a { color: #0366d6; text-decoration: none; cursor: pointer; }
          a.rid-link { color: #0366d6; cursor: pointer; text-decoration: underline; }
          nav a { margin-right: 1rem; color: #0366d6; }
        </style>
      </head>
      <body>
        <nav><a href="/events">Events</a></nav>
        <%= @inner_content %>
      </body>
    </html>
    """
  end
end
