"""Optional browser smoke test. Requires Python Playwright and a running preview server."""
import os
import sys
from playwright.sync_api import sync_playwright, expect

base_url = os.environ.get("PHL_PREVIEW_URL", "http://127.0.0.1:5189")
errors = []
with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)
    try:
        page = browser.new_page(viewport={"width": 1440, "height": 960})
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.on("console", lambda message: errors.append(message.text) if message.type == "error" else None)
        page.goto(base_url)
        page.wait_for_load_state("networkidle")
        page.get_by_role("button", name="跳过", exact=True).click()
        for label, path in [("版本", "versions"), ("插件", "plugins"), ("API", "api-config"),
                            ("运行时", "runtimes"), ("设置", "settings"), ("实例", "instances")]:
            page.get_by_role("button", name=label, exact=True).first.click()
            expect(page).to_have_url(f"{base_url}/#/{path}")
            page.wait_for_load_state("networkidle")
            expect(page.locator("main")).not_to_be_empty()
            expect(page.get_by_role("status").filter(has_text="加载页面")).to_have_count(0)
            print(f"PASS navigation: {path}")
        page.get_by_role("button", name="新建实例", exact=True).first.click()
        expect(page).to_have_url(f"{base_url}/#/create")
        expect(page.locator("main")).not_to_be_empty()
        expect(page.get_by_role("status").filter(has_text="加载页面")).to_have_count(0)
        print("PASS create page")
        # Also verify direct navigation to a lazy detail module with no matching instance.
        page.goto(f"{base_url}/#/instance/missing-smoke-test")
        page.wait_for_load_state("networkidle")
        expect(page.locator("main")).not_to_be_empty()
        expect(page.get_by_role("status").filter(has_text="加载页面")).to_have_count(0)
        print("PASS missing-instance detail page")
        if errors:
            raise AssertionError("Browser errors: " + "\n".join(errors))
        print("PASS no browser console errors")
    finally:
        browser.close()
sys.exit(0)
