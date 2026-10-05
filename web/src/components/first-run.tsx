// The "nothing has been sent yet" state, taught the same way on every page:
// what will show up here, and the two ways to get data (connect an app, or load demo data).

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { LoadDemoButton } from "@/pages/Onboarding";
import { Button } from "@/components/ui/button";
import { usePath, useProjectId } from "@/lib/context";
import { statusQuery } from "@/lib/queries";
import type { IconName } from "./icons";
import { Empty } from "./feedback";

/** True only once the server has said the project has never stored an event. */
export function useFirstRun(): boolean {
  return useQuery(statusQuery(useProjectId())).data?.has_events === false;
}

export function FirstRunEmpty({ icon, title, children }: { icon: IconName; title: string; children: ReactNode }) {
  const path = usePath();
  return (
    <Empty
      icon={icon}
      title={title}
      action={
        <div className="flex flex-wrap items-center justify-center gap-2">
          <LoadDemoButton />
          <Button nativeButton={false} render={<Link to={path("onboarding")} />}>
            Connect your app
          </Button>
        </div>
      }
    >
      {children}
    </Empty>
  );
}
