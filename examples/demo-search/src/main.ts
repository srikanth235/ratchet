import { getCatalog } from "./data.js";
import { createServer } from "./server.js";

const port = Number(process.env.PORT) || 3000;
const catalog = getCatalog();
createServer(catalog).listen(port, () => {
  console.log(`demo-search listening on :${port} (${catalog.products.length} products)`);
});
