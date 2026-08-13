import { resolveBase, connect } from "./api.js";
import { Console } from "./console.js";
import { PianoKeys } from "./keys.js";

const base = await resolveBase();
let send;
const view = new Console(document, (query) => send(query));
const keys = new PianoKeys(document, (query) => send(query));
send = connect(
  base,
  (snapshot) => {
    view.render(snapshot);
    keys.update(snapshot);
  },
  (message) => view.offline(message),
);
