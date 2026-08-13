import { resolveBase, connect } from "./api.js";
import { Console } from "./console.js";

const base = await resolveBase();
let send;
const view = new Console(document, (query) => send(query));
send = connect(
  base,
  (snapshot) => view.render(snapshot),
  (message) => view.offline(message),
);
